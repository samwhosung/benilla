// The retained static-world pass: `wow_model.wgsl`'s shading for statics that never fade or
// animate (ADT doodads, WMO groups, interior M2 props), with per-vertex flag words and a per-item
// record in place of materials. `StaticGx::divert` admits opaque and alpha-tested batches with no
// env map and no depth flags. The light prefix, helpers and lighting lanes copy `wow_model.wgsl`'s
// and must stay in sync: naga_oil cannot import functions that use another module's bindings.

#import bevy_render::view::View

@group(0) @binding(0) var<uniform> view: View;

// Prefix of lighting::global_light's buffer; field order in sync with it and wow_model.wgsl.
struct WowLight {
    light_ambient: vec4<f32>,
    light_diffuse: vec4<f32>,
    light_sun: vec4<f32>,
    light_spec: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    sh_c10_r: vec4<f32>,
    sh_c10_g: vec4<f32>,
    sh_c10_b: vec4<f32>,
    sh_c13_r: vec4<f32>,
    sh_c13_g: vec4<f32>,
    sh_c13_b: vec4<f32>,
    sh_c16: vec4<f32>,
    _water: array<vec4<f32>, 4>,
    grade: vec4<f32>,
    wmo_fog_color: vec4<f32>,
    wmo_fog_params: vec4<f32>,
    point_count: vec4<f32>,
    points: array<vec4<f32>, 512>,
    // lighting::prop_probes: 8192 slots of 7 rows; the buffer's later regions are not mirrored.
    prop_probes: array<vec4<f32>, 57344>,
}
@group(0) @binding(1) var<storage, read> wow_light: WowLight;

// Per-cell state (static_gx/render.rs `cell_layout`).
struct GxCell {
    origin: vec4<f32>, // xyz = the bake's recentring origin
}
@group(1) @binding(0) var<uniform> cell: GxCell;
// The per-item records, indexed by the vertex word's low 16 bits: x the texture-array layer, y
// the WMO batch order (0 on cells), z the MOMT SIDN colour r|g<<8|b<<16 in gamma bytes, w flags:
// bit 0 exile kill, bits 1..=13 the interior-prop probe slot, bit 14 interior fog (`[0xca7f00]`).
@group(1) @binding(1) var<storage, read> recs: array<vec4<u32>>;
@group(1) @binding(2) var tex_array: texture_2d_array<f32>;
// Repeat and clamp model-albedo samplers: trilinear, aniso 8, the same as the entity path's.
@group(1) @binding(3) var samp_repeat: sampler;
@group(1) @binding(4) var samp_clamp: sampler;

// Vertex word bits; keep in sync with static_gx/mod.rs WORD_*.
const WORD_WRAP_X: u32 = 65536u;    // 1 << 16
const WORD_WRAP_Y: u32 = 131072u;   // 1 << 17
const WORD_UNLIT: u32 = 262144u;    // 1 << 18
const WORD_FOG_OFF: u32 = 524288u;  // 1 << 19
const WORD_SHADE_LIT: u32 = 1048576u; // 1 << 20: ShadeSel::Lit, which no static carries
const WORD_TEXTURED: u32 = 2097152u;  // 1 << 21
// The WMO lane: the entity path's per-material facts as bits.
const WORD_WMO: u32 = 4194304u;        // 1 << 22: model_flags.x, a WMO surface
const WORD_INTERIOR: u32 = 8388608u;   // 1 << 23: model_flags.z, an interior group
const WORD_CLASS_INT: u32 = 16777216u; // 1 << 24: tint.w == 1, an INT batch
const WORD_CLASS_TRANS: u32 = 33554432u; // 1 << 25: tint.w == 2, a TRANS batch
const WORD_WINDOW: u32 = 67108864u;    // 1 << 26: sidn.w, the WINDOW midpoint light
const WORD_HAS_VC: u32 = 134217728u;   // 1 << 27: the batch authors vertex colours
// INTERIOR without WMO is an interior M2 prop, the entity shader's `interior_prop =
// flags.z && !flags.x`: probe lighting, interior fog, no point lights.
const WORD_MATTE: u32 = 268435456u;    // 1 << 28: ShadeSel::Matte, fixed intensity 1.0

// The alpha-test cutout reference 224/255, as wow_model.wgsl's VANILLA_ALPHA_KEY.
const VANILLA_ALPHA_KEY: f32 = 0.8784314;

// ---- mirrored law (wow_model.wgsl) ----

fn wow_normalize(v: vec3<f32>) -> vec3<f32> {
    let l2 = dot(v, v);
    return select(vec3<f32>(0.0), normalize(v), l2 > 1e-12);
}

fn point_light_sum(P: vec3<f32>, N: vec3<f32>, anchor: vec3<f32>) -> vec4<f32> {
    let count = u32(wow_light.point_count.x);
    var sel = array<u32, 3>(0u, 0u, 0u);
    var sd = array<f32, 3>(1e30, 1e30, 1e30);
    for (var i = 0u; i < count; i = i + 1u) {
        let pos_range = wow_light.points[2u * i];
        let dv = pos_range.xyz - anchor;
        let d2 = dot(dv, dv);
        if (d2 > pos_range.w * pos_range.w) {
            continue;
        }
        if (d2 < sd[0]) {
            sd[2] = sd[1]; sel[2] = sel[1];
            sd[1] = sd[0]; sel[1] = sel[0];
            sd[0] = d2; sel[0] = i;
        } else if (d2 < sd[1]) {
            sd[2] = sd[1]; sel[2] = sel[1];
            sd[1] = d2; sel[1] = i;
        } else if (d2 < sd[2]) {
            sd[2] = d2; sel[2] = i;
        }
    }
    var sum = vec4<f32>(0.0);
    for (var s = 0u; s < 3u; s = s + 1u) {
        if (sd[s] > 9.9e29) {
            break;
        }
        let pos_range = wow_light.points[2u * sel[s]];
        let to_light = pos_range.xyz - P;
        let d = length(to_light);
        let atten = 1.0 / (0.7 * d + 0.03 * d * d);
        let nl = max(dot(N, to_light / max(d, 1e-4)), 0.0);
        sum += vec4<f32>(wow_light.points[2u * sel[s] + 1u].rgb * (atten * nl), 1.0);
    }
    return sum;
}

// The lit colour at one vertex, `wow_model.wgsl`'s `vertex_light` over the flag words: the
// reference lights per vertex and interpolates the clamped colour, the vertex colour inside the
// clamp as the lit material. Both faces light from the submitted normal (no
// GL_LIGHT_MODEL_TWO_SIDE in the reference). `vc` is the vertex colour, white where the batch
// authors none; `point_lit.w` counts the bound point lights.
fn vertex_light(normal: vec3<f32>, point_lit: vec4<f32>, vc: vec4<f32>, word: u32) -> vec3<f32> {
    let n_lit = wow_normalize(normal);
    let L = -normalize(wow_light.light_sun.xyz);
    let ndotl = max(dot(n_lit, L), 0.0);
    // The FFP light sum, unclamped: GL clamps the lit colour once, after the material colour.
    let sum_nl = wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * ndotl;
    let rec = recs[word & 0xffffu];
    if ((word & WORD_WMO) != 0u) {
        // ---- WMO surfaces: wow_model.wgsl's is_wmo lane ----
        // Interior batch classes: INT is unlit (the bake is the room's light), TRANS's lit pass
        // and EXT take the FFP sum. Exterior groups take it at sun scale 1, no terrain shade and
        // no SH lobe (prog 198/VS 151). WINDOW (MOMT 0x20), interior drawer only: GL_LIGHT0
        // becomes the Direct/Ambient midpoint as diffuse and it +16/255 as ambient, that byte
        // add saturating on its own (0x6d37e0).
        let window_mid = 0.5 * (wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb);
        let sum_window = min(window_mid + vec3<f32>(16.0 / 255.0), vec3<f32>(1.0))
            + window_mid * ndotl;
        var sum_wmo = sum_nl;
        // SIDN night glow (MOMT 0x10): authored emissive × night fraction, an emission term
        // inside the clamp and never MOCV-multiplied; zero on INT, and TRANS's blend weights it.
        var sidn_w = 1.0;
        if ((word & WORD_INTERIOR) != 0u) {
            sum_wmo = select(sum_nl, sum_window, (word & WORD_WINDOW) != 0u);
            if ((word & WORD_CLASS_INT) != 0u) {
                sum_wmo = vec3<f32>(1.0);
                sidn_w = 0.0;
            }
        }
        let sidn_rgb = vec3<f32>(
            f32(rec.z & 0xffu),
            f32((rec.z >> 8u) & 0xffu),
            f32((rec.z >> 16u) & 0xffu),
        ) / 255.0;
        let sidn_e = sidn_rgb * (wow_light.grade.x * sidn_w);
        // GL_COLOR_MATERIAL: MOCV multiplies the light sum inside the clamp; emission adds beside.
        return clamp(vc.rgb * sum_wmo + sidn_e, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    // Order-2 SH basis terms of the vertex normal, for the doodad lobe and the probe lane.
    let quad = vec4<f32>(n_lit.x * n_lit.y, n_lit.y * n_lit.z, n_lit.z * n_lit.z, n_lit.x * n_lit.z);
    let x2y2 = n_lit.x * n_lit.x - n_lit.y * n_lit.y;
    var e: vec3<f32>;
    if ((word & WORD_INTERIOR) != 0u) {
        // ---- interior M2 props: wow_model.wgsl's interior-prop lane ----
        // The spawn-folded SH probe (MODD ambient, fixed-axis diffuse, the group's MOLR lobes)
        // over the vertex normal; its soft wrap is the reference's, not a hard max(N·L, 0).
        let probe = 7u * ((rec.w >> 1u) & 0x1fffu);
        let n1 = vec4<f32>(n_lit, 1.0);
        e = vec3<f32>(
            dot(wow_light.prop_probes[probe + 0u], n1)
                + dot(wow_light.prop_probes[probe + 3u], quad)
                + wow_light.prop_probes[probe + 6u].x * x2y2,
            dot(wow_light.prop_probes[probe + 1u], n1)
                + dot(wow_light.prop_probes[probe + 4u], quad)
                + wow_light.prop_probes[probe + 6u].y * x2y2,
            dot(wow_light.prop_probes[probe + 2u], n1)
                + dot(wow_light.prop_probes[probe + 5u], quad)
                + wow_light.prop_probes[probe + 6u].z * x2y2,
        );
    } else {
        // ---- exterior ADT doodads and exterior MODD props ----
        // Model2.bls order-2 SH sun lobe at intensity 0.5 Shaded (MCSH), 1.0 Matte, 2.5 Lit.
        // Deviation: the `min(I, 1)` cap, as in wow_model.wgsl, because lifting it takes
        // sun-facing surfaces past 1.0; Matte keeps its own bit so it stays 1.0 if the cap goes.
        let shade_t = select(1.0, 0.0, (word & WORD_SHADE_LIT) != 0u);
        let intensity = min(
            select(mix(2.5, 0.5, shade_t), 1.0, (word & WORD_MATTE) != 0u),
            1.0,
        );
        let sun_dc = wow_light.grade.yzw * intensity;
        let sun_lobe = vec3<f32>(
            wow_light.sh_c10_r.w + sun_dc.x
                + intensity
                    * (dot(wow_light.sh_c10_r.xyz, n_lit) + dot(wow_light.sh_c13_r, quad)
                        + wow_light.sh_c16.x * x2y2),
            wow_light.sh_c10_g.w + sun_dc.y
                + intensity
                    * (dot(wow_light.sh_c10_g.xyz, n_lit) + dot(wow_light.sh_c13_g, quad)
                        + wow_light.sh_c16.y * x2y2),
            wow_light.sh_c10_b.w + sun_dc.z
                + intensity
                    * (dot(wow_light.sh_c10_b.xyz, n_lit) + dot(wow_light.sh_c13_b, quad)
                        + wow_light.sh_c16.z * x2y2),
        );
        // Sun disabled (light_sun.w) falls back to the FFP sum, like the entity path.
        e = select(sum_nl, sun_lobe, wow_light.light_sun.w > 0.5);
    }
    // `Model2.bls`: with a point light bound the program clamps E to [0, 1] before `c28`
    // multiplies it, with none it writes `c28·E` to the saturating `oD0`.
    e += point_lit.rgb;
    if (point_lit.w > 0.5) {
        e = clamp(e, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    // `c28`: the vertex colour, the M2Color baked constant, is the lit material.
    return clamp(vc.rgb * e, vec3<f32>(0.0), vec3<f32>(1.0));
}

// ---- the pass ----

struct GxVertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) word: u32,
    @location(4) anchor: vec3<f32>,
    // MOCV or the baked constant tint; white where the batch authors none (WORD_HAS_VC).
    @location(5) color: vec4<f32>,
}

struct GxVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) @interpolate(flat) word: u32,
    // The lit colour from `vertex_light`, clamped at the vertex and interpolated.
    @location(4) primary: vec3<f32>,
    @location(5) color: vec4<f32>,
}

@vertex
fn vertex(v: GxVertex) -> GxVsOut {
    var out: GxVsOut;
    // Exile kill bit (record w bit 0): every vertex of the item collapses to one point, so its
    // triangles are zero-area and rasterize nothing; triangles never span items.
    if ((recs[v.word & 0xffffu].w & 1u) != 0u) {
        out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.world_position = vec4<f32>(0.0);
        out.uv = vec2<f32>(0.0);
        out.word = v.word;
        out.primary = vec3<f32>(0.0);
        out.color = vec4<f32>(1.0);
        return out;
    }
    // Camera-relative for f32 precision: the recentred vertex plus (cell origin - camera).
    let p_cam = v.position + (cell.origin.xyz - view.world_position);
    let world = v.position + cell.origin.xyz;
    out.world_position = vec4<f32>(world, 1.0);
    let view_rot = mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    );
    out.position = view.clip_from_view * vec4<f32>(view_rot * p_cam, 1.0);
    // Authored MOBA batch order (record y, 0 on cells): a later coplanar batch wins reverse-Z
    // GreaterEqual in any draw order, because the draw sort does not keep authored order.
    out.position.z *= 1.0 + f32(recs[v.word & 0xffffu].y) * 1.1920929e-7;
    out.uv = v.uv;
    out.word = v.word;
    out.color = v.color;
    // Up to 3 nearest point lights, chosen from the placement origin. WMO surfaces take none,
    // as in the reference; interior props neither: the probe holds their group's MOLR lights.
    var point_lit = vec4<f32>(0.0);
    if ((v.word & (WORD_WMO | WORD_INTERIOR)) == 0u) {
        point_lit = point_light_sum(world, v.normal, v.anchor);
    }
    // A colour-less batch takes a constant 1.0 (WORD_HAS_VC).
    let vc = select(vec4<f32>(1.0), v.color, (v.word & WORD_HAS_VC) != 0u);
    out.primary = vertex_light(v.normal, point_lit, vc, v.word);
    return out;
}

@fragment
fn fragment(in: GxVsOut) -> @location(0) vec4<f32> {
    // The hard farclip wall: per-pixel planar eye-Z, the same plane as the entity path.
    let eye_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
    if (wow_light.fog_params.w > 0.0 && eye_z > wow_light.fog_params.w) {
        discard;
    }
    // The sampler follows the wrap flags; a mixed batch clamps its clamped axis a half texel in.
    // Every sample runs unconditionally: implicit derivatives need uniform control flow.
    var base = vec4<f32>(1.0);
    let wrap_x = (in.word & WORD_WRAP_X) != 0u;
    let wrap_y = (in.word & WORD_WRAP_Y) != 0u;
    if ((in.word & WORD_TEXTURED) != 0u) {
        let layer = i32(recs[in.word & 0xffffu].x);
        let dims = vec2<f32>(textureDimensions(tex_array).xy);
        let inset = 0.5 / dims;
        var uv_mixed = in.uv;
        if (!wrap_x) {
            uv_mixed.x = clamp(uv_mixed.x, inset.x, 1.0 - inset.x);
        }
        if (!wrap_y) {
            uv_mixed.y = clamp(uv_mixed.y, inset.y, 1.0 - inset.y);
        }
        // The render-scale LOD bias: bevy's material path applies it for free, this lane by hand.
        let c_repeat = textureSampleBias(tex_array, samp_repeat, in.uv, layer, view.mip_bias);
        let c_clamp = textureSampleBias(tex_array, samp_clamp, in.uv, layer, view.mip_bias);
        let c_mixed = textureSampleBias(tex_array, samp_repeat, uv_mixed, layer, view.mip_bias);
        if (wrap_x && wrap_y) {
            base = c_repeat;
        } else if (!wrap_x && !wrap_y) {
            base = c_clamp;
        } else {
            base = c_mixed;
        }
    }
#ifdef GX_CUTOUT
    if (base.a < VANILLA_ALPHA_KEY) {
        discard;
    }
#endif
    // A colour-less batch takes a constant 1.0, not interpolated white: interpolating a constant
    // attribute gives 1.0±ε, and base × 0.99999994 rounds about half the pixels one byte down.
    let has_vc = (in.word & WORD_HAS_VC) != 0u;
    let vc = select(vec4<f32>(1.0), in.color, has_vc);
    let folded = select(base.rgb, base.rgb * vc.rgb, has_vc);
    // The stage-0 modulate: the texel times the lit colour `vertex_light` clamped per vertex,
    // whose vertex colour is inside its clamp.
    var rgb = base.rgb * in.primary;
    if ((in.word & WORD_WMO) != 0u && (in.word & WORD_INTERIOR) != 0u) {
        if ((in.word & WORD_CLASS_TRANS) != 0u) {
            // TRANS: the lit pass by MOCV.a (blend 9) under the unlit `tex × MOCV` by 1 − MOCV.a
            // (blend 7), `0x6b5190`, as one lerp.
            rgb = base.rgb * mix(vc.rgb, in.primary, vc.a);
        } else if ((in.word & WORD_CLASS_INT) != 0u && has_vc) {
            // INT self-illumination, the reference's interior pixel shader (literal 4.0):
            // tex·MOCV·(1 + 4·MOCV.a), clamped once. A colour-less INT batch keeps tex × lit.
            rgb = clamp(
                base.rgb * vc.rgb * (1.0 + 4.0 * vc.a),
                vec3<f32>(0.0),
                vec3<f32>(1.0),
            );
        }
    }
    // Unlit (M2 UNLIT 0x01, WMO UNLIT on an exterior group): texture × vertex colour, no light.
    if ((in.word & WORD_UNLIT) != 0u) {
        rgb = folded;
    }
    // Planar eye-Z fog; other fog modes belong to blends never admitted here. The interior triple
    // keys on the per-frame record bit, not `WORD_INTERIOR`: the client sets it per group under
    // `[0xca7f00]` (`0x6b5190` for surfaces, `0x6b62e0` for the group's doodads).
    var fog_color = wow_light.fog_color;
    var fog_span = wow_light.fog_params.xy;
    if ((recs[in.word & 0xffffu].w & 16384u) != 0u) {
        fog_color = wow_light.wmo_fog_color;
        fog_span = wow_light.wmo_fog_params.xy;
    }
    if (fog_color.w > 0.5 && (in.word & WORD_FOG_OFF) == 0u) {
        let denom = max(fog_span.y - fog_span.x, 0.001);
        let factor = clamp((fog_span.y - eye_z) / denom, 0.0, 1.0);
        rgb = mix(fog_color.xyz, rgb, factor);
    }
    // Gamma-space output; alpha pinned 1.0, every draw here is opaque.
    return vec4<f32>(rgb, 1.0);
}
