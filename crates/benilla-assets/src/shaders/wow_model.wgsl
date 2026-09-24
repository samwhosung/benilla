// The model pass for M2, WMO and ground-clutter meshes, lit and fogged in gamma space:
//   M2:           color = clamp01(A + D·I·(4/17)(0.375 + 2μ + 1.875μ²)) × tex × tint, μ = N·L
//   clutter, WMO: color = clamp(ambient + diffuse·max(N·L, 0)) × tex × tint
//   fog:          color = mix(fog_color, color, fog_factor); out = color, raw gamma
// The M2 law is the order-2 SH lobe of `Shaders\Vertex\Model2.bls` (cvar `M2UseShaders`, default
// 1); M2's one FFP light site (`70bdf6`) runs only with that cvar off. Clutter and WMO are the FFP
// light (GL_LIGHTING, GL_LIGHT0, GL_COLOR_MATERIAL); clutter's normal is the terrain normal under
// the tuft, which the reference writes onto the clutter vertex.
// Not built: a specular term (the M2 per-material shininess is only inferred) and the WMO
// per-group authored colour.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    pbr_bindings,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
    mesh_functions,
}

// bevy_pbr 0.18.1's `forward_io::FragmentOutput`. No depth output: a fragment depth write costs
// the pipeline early-Z, so the sky lane pins its depth in the vertex stage.
struct WowFragOut {
    @location(0) color: vec4<f32>,
}

// Per-material uniforms at binding 100, in `WowModelExt`'s field order (materials.rs).
//   clutter_fade: x = plateau-end view depth (yd, 0.75·far), y = ramp-zero view depth (the ~70 yd
//     detail-doodad horizon `[0x867958]`), z = the batch marker bits, w = clutter
//   model_flags: x = WMO, y = fade blend twin, z = interior (a WMO interior group, or an interior
//     M2 lit by its SH probe), w = unlit fullbright (M2 UNLIT 0x01 or Mod/Mod2x, or WMO UNLIT on
//     an exterior-group batch; the interior drawer ignores it)
struct ModelParams {
    clutter_fade: vec4<f32>,
    model_flags: vec4<f32>,
    // x = the terrain-shade selector (see the doodad sun), y = the WMO batch order, zw = the
    // UV-scroll seed.
    sun_scale: vec4<f32>,
    // xyz = the animated M2Color tint (identity when static); w = the WMO interior batch class:
    // 0 exterior law, 1 INT, 2 TRANS.
    tint: vec4<f32>,
    // WMO glass, 0 on M2: xyz = the MOMT SIDN (0x10) emissive (gamma /255), w = MOMT WINDOW (0x20).
    sidn: vec4<f32>,
    // Rows of `wow_light.matanim`, 0 = identity: x = UV scroll, y = tint, z = texture-transform
    // affine, w = the UI tile's cell clip.
    anim_slots: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> m: ModelParams;

// The shared light buffer (`lighting::global_light`, rows as packed there), updated once per
// frame; terrain.wgsl mirrors the prefix, so a tree and the ground under it fog alike.
struct WowLight {
    light_ambient: vec4<f32>, // rgb ambient; w = Mod2x scale
    light_diffuse: vec4<f32>, // rgb sun diffuse; w = clamp-light flag (>0.5 ⇒ saturate)
    light_sun: vec4<f32>,     // xyz sun travel dir (to-light = −xyz); w = directional enable
    light_spec: vec4<f32>,    // terrain's specular (w = its shininess); unread here
    fog_color: vec4<f32>,     // rgb row-7 fog (gamma); w = enable (>0.5)
    fog_params: vec4<f32>,    // x = start, y = end, w = farclip wall; z unused
    // Rows 6-12: the reference's `Model2.bls` order-2 SH probe of the scene light at intensity 1:
    // ambient DC in the c10 `.w` lanes, the sun's bands in c10.xyz, c13 and c16.xyz. Every sun band
    // is linear in the committed colour, so all scale by the intensity (never I²).
    sh_c10_r: vec4<f32>,
    sh_c10_g: vec4<f32>,
    sh_c10_b: vec4<f32>,
    sh_c13_r: vec4<f32>,
    sh_c13_g: vec4<f32>,
    sh_c13_b: vec4<f32>,
    // xyz = the c16 quadratic band (x²−y²) per channel; w unused.
    sh_c16: vec4<f32>,
    _water: array<vec4<f32>, 4>, // rows 13-16: the liquid swatches, unread here
    // x = the SIDN night fraction (1 overnight, 0 by day); yzw = the sun's SH DC at intensity 1.
    grade: vec4<f32>,
    // Rows 18-19: the interior fog, the 4 s camera-in-WMO MFOG crossfade (the scene fog outdoors).
    wmo_fog_color: vec4<f32>,    // rgb interior fog (gamma); w = enable (mirrors fog_color.w)
    wmo_fog_params: vec4<f32>,   // x = start yd, y = end yd; zw unused
    // Row 20 `.x` = the point-light count, then per light `[pos.xyz, range]`, `[rgb, 0]`. Here, not
    // in Bevy's clusterables, which the view layout gives the fragment stage only.
    point_count: vec4<f32>,
    points: array<vec4<f32>, 512>,
    // The interior-prop SH probes, 7 rows per slot (`MAX_PROP_PROBES` = 8192 slots). Only this
    // shader declares this tail; the other shaders bind the same buffer by its prefix.
    prop_probes: array<vec4<f32>, 57344>,
    // Per rig slot (sizes mirrored in rig_palette.rs): the base bone index into `palettes`, whose
    // 3 rows per bone are `rig_from_joint × inverse_bindpose` from the rig's own origin.
    rig_table: array<u32, 2048>,
    // Per rig slot: the CM2 body tint (`model+0x184/188/18c`) packed `0xFFRRGGBB` like the
    // reference's node value (`0x60d840`: `param | 0xff000000`); 0 is identity.
    rig_tint: array<u32, 2048>,
    // Per rig slot: the world origin its palette rows are measured from.
    rig_origin: array<vec4<f32>, 2048>,
    // The mat-anim rows (size mirrored in mat_anim_table.rs), row 0 zero: a UV-scroll delta (xy),
    // a tint delta (xyz), a texture-transform affine `[cos − 1, sin, sx − 1, sy − 1]` or a UI cell.
    matanim: array<vec4<f32>, 2048>,
    // Per rig slot, the straddle waterline (size mirrored in straddle.rs): x = its world height
    // (Bevy Y), y = the side the near copy keeps (+1 above, −1 below, 0 not straddling).
    water_clip: array<vec2<f32>, 2048>,
    palettes: array<vec4<f32>>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

// The M2 cutout alpha-test reference, 224/255; in sync with `materials::VANILLA_ALPHA_KEY_REF`.
const VANILLA_ALPHA_KEY: f32 = 0.8784314;

// The detail-doodad pass's stage-0 `D3DSAMP_MIPMAPLODBIAS = +0.25` (`0x6813f4`).
const DETAIL_DOODAD_LOD_BIAS: f32 = 0.25;

// Bevy's `VertexOutput` (same fields, locations and defs; no tangents, morphs or visibility
// ranges) plus our interpolants; the fragment rebuilds a `VertexOutput` from it.
struct WowVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv_b: vec2<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(6) @interpolate(flat) instance_index: u32,
#endif
    // The point-light sum `Σ att·sat(N·L)·colour`, per vertex like the reference FFP.
    @location(8) point_lit: vec3<f32>,
#ifdef WOW_MERGED_FADE
    // A merged blob's per-placement fade alpha, constant over the placement.
    @location(9) merged_fade: f32,
#endif
#ifdef WOW_MERGED_SLOT
    @location(10) @interpolate(flat) merged_slot: u32,
#endif
}

// Zero-safe normalize. M2s author `(0,0,0)` normals (`Creature\QuirajProphet`) and the reference
// lights them: `Model2.bls` normalizes with `RSQ`/`MUL`, where `0 × INF` is 0, leaving the SH DC
// term. `normalize(0)` is NaN, which clamps to 0 and blacks the batch. Wrapping `normalize`
// rather than `v · inverseSqrt(l2)` keeps every nonzero normal's bits.
fn wow_normalize(v: vec3<f32>) -> vec3<f32> {
    let l2 = dot(v, v);
    return select(vec3<f32>(0.0), normalize(v), l2 > 1e-12);
}

// The point lights at P: the reference commits at most three per draw, the nearest to the
// receiving unit's own position `anchor` (`0x71bf90` gathers by squared distance, `0x71c730` seats
// slots 1-3). Range bounds candidacy only; a selected light falls off as `1/(0.7·d + 0.03·d²)`,
// diffuse only, on the submitted normal (no GL_LIGHT_MODEL_TWO_SIDE). Mirrored in terrain.wgsl.
fn point_light_sum(P: vec3<f32>, N: vec3<f32>, anchor: vec3<f32>) -> vec3<f32> {
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
    var sum = vec3<f32>(0.0);
    for (var s = 0u; s < 3u; s = s + 1u) {
        if (sd[s] > 9.9e29) {
            break;
        }
        let pos_range = wow_light.points[2u * sel[s]];
        let to_light = pos_range.xyz - P;
        let d = length(to_light);
        let atten = 1.0 / (0.7 * d + 0.03 * d * d);
        let nl = max(dot(N, to_light / max(d, 1e-4)), 0.0);
        sum += wow_light.points[2u * sel[s] + 1u].rgb * (atten * nl);
    }
    return sum;
}

// The centre of the MCNK chunk under P, the light anchor for world-merged clutter, since the
// reference gathers lights per terrain chunk (mirrored in terrain.wgsl). WoW x/y are Bevy −z/−x
// and the grid is symmetric, so snapping Bevy x/z lands on the same cells. The height is the
// vertex's own, not the chunk record's: lights sit near the surface.
fn mcnk_cell_anchor(P: vec3<f32>) -> vec3<f32> {
    let cell = 533.33333 / 16.0;
    let half = 32.0 * 533.33333;
    let ix = floor((half + P.x) / cell);
    let iz = floor((half + P.z) / cell);
    return vec3<f32>((ix + 0.5) * cell - half, P.y, (iz + 0.5) * cell - half);
}

#ifdef WOW_MERGED_FADE
// The doodad fade curve (`FUN_00683f80`, in sync with `model_fade::doodad_fade_alpha`): alpha =
// 1 − (d − start)/range, d = horizontal distance − radius, over a size-bucketed band.
fn merged_fade_alpha(radius: f32, horiz_dist: f32) -> f32 {
    if (radius > 7.0) {
        return 1.0;
    }
    var start = 150.0;
    var range = 50.0;
    if (radius <= 0.5) {
        start = 40.0;
        range = 10.0;
    } else if (radius <= 2.5) {
        start = 100.0;
        range = 25.0;
    }
    let d = horiz_dist - radius;
    return clamp(1.0 - (d - start) / range, 0.0, 1.0);
}
#endif

// Bevy 0.18's `forward_io::Vertex` at Bevy's locations, plus the palette joints at 10/11 (appended
// by `WowModelExt::specialize` for a mesh with `ATTRIBUTE_WOW_JOINT_INDEX`) and the merged-blob
// attributes at 12/13.
struct WowVertex {
    @builtin(instance_index) instance_index: u32,
#ifdef VERTEX_POSITIONS
    @location(0) position: vec3<f32>,
#endif
#ifdef VERTEX_NORMALS
    @location(1) normal: vec3<f32>,
#endif
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv_b: vec2<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
#ifdef WOW_RIG_SKIN
    @location(10) joint_indices: vec4<u32>,
    @location(11) joint_weights: vec4<f32>,
#endif
#ifdef WOW_MERGED_FADE
    // The placement fade sphere: xyz = world centre, w = fade radius.
    @location(12) fade_sphere: vec4<f32>,
#endif
#ifdef WOW_MERGED_SLOT
    // The interior-prop SH-probe slot, in place of the per-entity MeshTag payload.
    @location(13) merged_slot: u32,
#endif
}

#ifdef WOW_RIG_SKIN
fn wow_rig_slot(instance_index: u32) -> u32 {
    return (mesh_functions::get_tag(instance_index) >> 19u) & 0x7ffu;
}

// Blends the four weighted bones' palette rows into `rig_from_local`, which replaces the mesh's
// world matrix like Bevy's `skin_model`; its translation is relative to `rig_origin[slot]`.
fn wow_skin_model(instance_index: u32, indices: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    let base = wow_light.rig_table[wow_rig_slot(instance_index)];
    let b0 = 3u * (base + indices.x);
    let b1 = 3u * (base + indices.y);
    let b2 = 3u * (base + indices.z);
    let b3 = 3u * (base + indices.w);
    let r0 = weights.x * wow_light.palettes[b0]
        + weights.y * wow_light.palettes[b1]
        + weights.z * wow_light.palettes[b2]
        + weights.w * wow_light.palettes[b3];
    let r1 = weights.x * wow_light.palettes[b0 + 1u]
        + weights.y * wow_light.palettes[b1 + 1u]
        + weights.z * wow_light.palettes[b2 + 1u]
        + weights.w * wow_light.palettes[b3 + 1u];
    let r2 = weights.x * wow_light.palettes[b0 + 2u]
        + weights.y * wow_light.palettes[b1 + 2u]
        + weights.z * wow_light.palettes[b2 + 2u]
        + weights.w * wow_light.palettes[b3 + 2u];
    // r0/r1/r2 are the affine's rows; a WGSL matrix is column-major.
    return mat4x4<f32>(
        vec4<f32>(r0.x, r1.x, r2.x, 0.0),
        vec4<f32>(r0.y, r1.y, r2.y, 0.0),
        vec4<f32>(r0.z, r1.z, r2.z, 0.0),
        vec4<f32>(r0.w, r1.w, r2.w, 1.0),
    );
}

// bevy_pbr::skinning's inverse-transpose via the adjugate, verbatim.
fn inverse_transpose_3x3m(in: mat3x3<f32>) -> mat3x3<f32> {
    let x = cross(in[1], in[2]);
    let y = cross(in[2], in[0]);
    let z = cross(in[0], in[1]);
    let det = dot(in[2], z);
    return mat3x3<f32>(x / det, y / det, z / det);
}

fn wow_skin_normals(frame_from_local: mat4x4<f32>, normal: vec3<f32>) -> vec3<f32> {
    return wow_normalize(
        inverse_transpose_3x3m(mat3x3<f32>(
            frame_from_local[0].xyz,
            frame_from_local[1].xyz,
            frame_from_local[2].xyz
        )) * normal
    );
}
#endif

// Bevy 0.18's `mesh.wgsl` vertex stage plus our skinning and point lights. A `MaterialExtension`
// replaces the whole stage, so this must track Bevy's on upgrades.
@vertex
fn vertex(vertex: WowVertex) -> WowVsOut {
    var out: WowVsOut;

    let mesh_world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    // Precision: `frame_from_local` holds the orientation and a small translation, `frame_origin`
    // the ~9 k yd world position, so no f32 product mixes the two (an f32 ULP at 9 k is ~1 mm).
#ifdef WOW_RIG_SKIN
    var frame_from_local = wow_skin_model(
        vertex.instance_index,
        vertex.joint_indices,
        vertex.joint_weights
    );
    let frame_origin = wow_light.rig_origin[wow_rig_slot(vertex.instance_index)].xyz;
#else
    var frame_from_local = mesh_world_from_local;
    let frame_origin = mesh_world_from_local[3].xyz;
    frame_from_local[3] = vec4<f32>(0.0, 0.0, 0.0, 1.0);
#endif

#ifdef VERTEX_NORMALS
#ifdef WOW_RIG_SKIN
    out.world_normal = wow_skin_normals(frame_from_local, vertex.normal);
#else
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index
    );
#endif
#endif

#ifdef VERTEX_POSITIONS
    // Precision: camera-relative to clip space; `clip_from_world × p_world` cancels
    // catastrophically with camera and geometry near 9 k yd. `world_position` is absolute again:
    // lighting and fog need no such precision.
    let p_cam = (frame_from_local * vec4<f32>(vertex.position, 1.0)).xyz
        + (frame_origin - view.world_position);
    out.world_position = vec4<f32>(p_cam + view.world_position, 1.0);
    let view_rot = mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    );
    out.position = view.clip_from_view * vec4<f32>(view_rot * p_cam, 1.0);
    // WMO batch order (`sun_scale.y`, 0 off WMO): the reference layers coplanar batches by MOBA
    // draw order under depth-write + LEQUAL; Bevy reorders draws, so a later batch must win the
    // reverse-Z GreaterEqual test. Scaling clip z by (1 + n·2⁻²³) raises z/w by n ULPs. Uniform
    // data, not a `DepthBiasState`, which would make every batch index its own pipeline.
    out.position.z *= 1.0 + m.sun_scale.y * 1.1920929e-7;
#ifdef WOW_SKY_DEPTH
    // The WMO-skybox lane (`clutter_fade.z` bit 13): clip z = 0 is reverse-Z infinitely far, so
    // the world always draws over the sky shell (`benilla_world::sky_order`).
    out.position.z = 0.0;
#endif
#ifdef WOW_MERGED_FADE
    // A fully faded placement leaves the clip volume, so its triangles never rasterize.
    let fade_d = distance(view.world_position.xz, vertex.fade_sphere.xz);
    out.merged_fade = merged_fade_alpha(vertex.fade_sphere.w, fade_d);
    if (out.merged_fade <= 0.0) {
        out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
    }
#ifdef WOW_MERGED_SLOT
    out.merged_slot = vertex.merged_slot;
#endif
#endif
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
    // Env-mapped batches (`clutter_fade.z` bit 12; `texture_unit_lookup[texCoordSet] > 2` at
    // `0x70b8bd`) generate their texcoord per vertex as `Model2.bls` does: in view space
    // R = normalize(P − 2(P·N)N), uv = R.xy·0.5 + 0.5 (`0x70b8d0`). The reference view basis
    // (`0x5c3e70`) and Bevy's −Z-forward one differ only in z, so R.xy needs no fixup.
#ifdef VERTEX_POSITIONS
#ifdef VERTEX_NORMALS
    if ((u32(m.clutter_fade.z) & 4096u) != 0u) {
        let p_view = view_rot * p_cam;
        let n_view = normalize(view_rot * out.world_normal);
        let refl = normalize(p_view - 2.0 * dot(p_view, n_view) * n_view);
        out.uv = refl.xy * 0.5 + vec2<f32>(0.5, 0.5);
    }
#endif
#endif
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

    // Point lights: none on WMO surfaces, none here on interior M2 props (their group's MOLR lights
    // are in the SH probe), else the ≤3 nearest to the instance origin (clutter: its MCNK chunk).
    if (m.model_flags.x > 0.5 || m.model_flags.z > 0.5) {
        out.point_lit = vec3<f32>(0.0);
    } else {
        var anchor = mesh_world_from_local[3].xyz;
        if (m.clutter_fade.w > 0.5) {
            anchor = mcnk_cell_anchor(out.world_position.xyz);
        }
        out.point_lit = point_light_sum(out.world_position.xyz, out.world_normal, anchor);
    }
    return out;
}

@fragment
fn fragment(in: WowVsOut, @builtin(front_facing) is_front: bool) -> WowFragOut {
    // The UI model tile's cell clip: the reference gives each `<Model>` pane its widget rect as the
    // viewport; our panes share an atlas, so the tile passes its cell as a mat-anim row
    // (`anim_slots.w`, `[min.x, min.y, max.x, max.y]` in atlas texels) and this is the scissor.
    if (m.anim_slots.w > 0.5) {
        let r = wow_light.matanim[u32(m.anim_slots.w)];
        if (in.position.x < r.x || in.position.y < r.y
            || in.position.x > r.z || in.position.y > r.w) {
            discard;
        }
    }
    // The far-clip wall: discard past `farclip` (`fog_params.w`, 0 = off) by planar eye depth, as
    // terrain.wgsl does.
    if (wow_light.fog_params.w > 0.0) {
        let clip_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        if (clip_z > wow_light.fog_params.w) {
            discard;
        }
    }
#ifdef WOW_WATER_CLIP
    // The straddle split: a translucent model crossing its water plane draws on each side of the
    // water pass (the far copy has `clutter_fade.z` bit 11) and each copy keeps its half, the
    // reference's `M2UseClipPlanes` plane at the waterline. In sync with `straddle::keeps`.
    let water_clip = wow_light.water_clip[(mesh_functions::get_tag(in.instance_index) >> 19u) & 0x7ffu];
    if (water_clip.y != 0.0) {
        let far_copy = (u32(m.clutter_fade.z) & 2048u) != 0u;
        let keep_side = select(water_clip.y, -water_clip.y, far_copy);
        if (keep_side * (in.world_position.y - water_clip.x) < 0.0) {
            discard;
        }
    }
#endif
    // Rebuild Bevy's `VertexOutput` with the M2 texture transform (in sync with
    // `tex_anim::uv_transform`): uv' = R((uv + t − p) ⊙ s) + p, p = (½, ½), t = `sun_scale.zw`
    // plus its mat-anim delta, R and s from the affine row `[cos − 1, sin, sx − 1, sy − 1]`.
    var vo: VertexOutput;
    vo.position = in.position;
    vo.world_position = in.world_position;
    vo.world_normal = in.world_normal;
#ifdef VERTEX_UVS_A
    // An env-mapped coordinate takes no UV animation: the reference excludes an env stage from
    // `textureTransform`.
    if ((u32(m.clutter_fade.z) & 4096u) != 0u) {
        vo.uv = in.uv;
    } else {
        let uv_t = in.uv + m.sun_scale.zw + wow_light.matanim[u32(m.anim_slots.x)].xy;
        let affine = wow_light.matanim[u32(m.anim_slots.z)];
        let d = (uv_t - vec2<f32>(0.5, 0.5)) * vec2<f32>(1.0 + affine.z, 1.0 + affine.w);
        let c = 1.0 + affine.x;
        vo.uv = vec2<f32>(0.5 + d.x * c - d.y * affine.y, 0.5 + d.x * affine.y + d.y * c);
    }
#endif
#ifdef VERTEX_UVS_B
    vo.uv_b = in.uv_b;
#endif
#ifdef VERTEX_COLORS
    vo.color = in.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    vo.instance_index = in.instance_index;
#endif
    var pbr_input = pbr_input_from_standard_material(vo, is_front);
    var base_color = pbr_input.material.base_color;
    // WMO MOCV alpha is lighting data (the TRANS lerp, the INT glow mask), and the reference's WMO
    // output alpha is tex.a alone. Bevy folds the vertex alpha into base_color, so re-sample the
    // texel alpha: dividing the fold back out loses everything where MOCV.a ≈ 0.
#ifdef VERTEX_COLORS
    if (m.model_flags.x > 0.5) {
#ifdef VERTEX_UVS_A
        base_color.a = textureSampleBias(
            pbr_bindings::base_color_texture,
            pbr_bindings::base_color_sampler,
            vo.uv,
            // The colour's LOD: `pbr_input_from_standard_material` applies `view.mip_bias`, and
            // coverage from another mip erodes out of step with the art.
            view.mip_bias,
        ).a;
#else
        base_color.a = 1.0;
#endif
    }
#endif
    // The detail-doodad LOD bias (`0x6813f4`): on the atlases whose alpha pyramid is binary below
    // mip 0 it keeps every fragment below full alpha, so the fade erodes per pixel, not per leaf.
    // wgpu has no sampler LOD bias and the sampler is shared with unbiased batches, so it goes on
    // this sample. Clutter's tint is white and its vertex colour the MCSH grey, so `* vo.color`
    // reproduces Bevy's fold.
    if (m.clutter_fade.w > 0.5) {
#ifdef VERTEX_UVS_A
        var biased = textureSampleBias(
            pbr_bindings::base_color_texture,
            pbr_bindings::base_color_sampler,
            vo.uv,
            view.mip_bias + DETAIL_DOODAD_LOD_BIAS,
        );
#ifdef VERTEX_COLORS
        biased = biased * vo.color;
#endif
        base_color = biased;
#endif
    }
    // The clutter distance fade, the reference's stage-1 ramp: u = (z_eye − 52.5)/17.5 by a
    // camera-space texgen (`0x6b2b80`), so the boundary is a view plane, not a sphere; the ramp is
    // a bilinear read of the 64-texel table `4·(63 − col)` (`0x6b2320`). It multiplies the alpha
    // the cutout tests (ALPHAREF `detailDoodadAlpha` = 128).
    if (m.clutter_fade.w > 0.5) {
        let z_eye = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        let u = (z_eye - m.clutter_fade.x) / max(m.clutter_fade.y - m.clutter_fade.x, 0.001);
        let ramp = clamp((254.0 - 256.0 * u) / 255.0, 0.0, 252.0 / 255.0);
        base_color.a = base_color.a * ramp;
    }

    // The MeshTag (mesh_tag.rs): bit 31 = highlight, bit 30 = interior fog; payload bits 0-5 = the
    // fade alpha (a zero payload is untagged, opaque), 19-29 = the rig slot, 6-13 = the ground
    // shade (0 lit, 255 MCSH-shadowed) or, on an interior-prop material, 6-18 = the SH-probe slot.
    let interior_prop = m.model_flags.z > 0.5 && m.model_flags.x < 0.5;
    let raw_tag = mesh_functions::get_tag(in.instance_index);
    let highlighted = (raw_tag & 0x80000000u) != 0u;
    let interior_fogged = (raw_tag & 0x40000000u) != 0u;
    let fade_tag = raw_tag & 0x3fffffffu;
    let alpha6 = f32(fade_tag & 0x3fu) / 63.0;
    var obj_fade = select(alpha6, 1.0, fade_tag == 0u);
#ifdef WOW_MERGED_FADE
    // A merged blob's per-vertex fade composes where the tag fade does, so it feathers the same.
    obj_fade = obj_fade * in.merged_fade;
#endif
    // The body tint (an aura colouring the whole model), by rig slot, 0 = identity: the material
    // ambient+diffuse colour (gx SetState(1), GL_COLOR_MATERIAL), so it multiplies the light sum
    // inside the clamp, never the emission.
    let tint_word = wow_light.rig_tint[(fade_tag >> 19u) & 0x7ffu];
    let inst_tint = select(
        vec3<f32>(
            f32((tint_word >> 16u) & 0xffu),
            f32((tint_word >> 8u) & 0xffu),
            f32(tint_word & 0xffu),
        ) * (1.0 / 255.0),
        vec3<f32>(1.0),
        tint_word == 0u,
    );
    // The blend twin re-applies its source's cutout: the reference scales ALPHAREF with the fade,
    // so the cutoff stays tex.a < 224/255 on the unfaded alpha. Only for a source batch that
    // alpha-tests (bit 10): ALPHAREF keys on the stored blend mode.
    if ((u32(m.clutter_fade.z) & 1024u) != 0u && base_color.a < VANILLA_ALPHA_KEY) {
        discard;
    }
    // The depth-prime twin (`M2UseZFill`) masks colour writes, so only the discards above shape its
    // depth. No early return: naga's MSL backend miscompiles the dead tail ("redefinition of
    // '_tmp'").
    let faded_alpha = base_color.a * obj_fade;
    let base = alpha_discard(pbr_input.material, base_color);

    // --- Lighting --------------------------------------------------------------------------------
    let is_clutter = m.clutter_fade.w > 0.5;
    let L = -normalize(wow_light.light_sun.xyz);
    // `wow_normalize`: an authored zero normal reaches here on the unskinned lane.
    let n_m2 = wow_normalize(pbr_input.world_normal);
    // Bevy negates `world_normal` on back faces of double-sided materials (foliage, every WMO
    // face); the reference never enables GL_LIGHT_MODEL_TWO_SIDE (`FUN_0059ce30`), so undo it.
    let n_lit = select(-n_m2, n_m2, is_front);
    let ndotl = max(dot(n_lit, L), 0.0);
    let lit_nl = clamp(wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * ndotl, vec3<f32>(0.0), vec3<f32>(1.0));
    // The order-2 SH basis, shared by the exterior lobe and the interior probes.
    let quad = vec4<f32>(n_lit.x * n_lit.y, n_lit.y * n_lit.z, n_lit.z * n_lit.z, n_lit.x * n_lit.z);
    let n1 = vec4<f32>(n_lit, 1.0);
    let x2y2 = n_lit.x * n_lit.x - n_lit.y * n_lit.y;
    // Exterior doodads and entities: the header's `Model2.bls` lobe at the per-instance intensity
    // I, `[node+0xa4]` (animator targets 2.5 lit and 0.5 shadowed, `0x69e4ad`/`0x69e280`; 1.0
    // indoors, `0x69e36b`). `sun_scale.x` picks the family:
    //   ≥ 0.85: an entity M2, 2.5 mixed toward 0.5 by the tag's shade byte (ramped like `0x69e770`)
    //   0.5..0.85: a doodad (ADT MDDF or WMO MODD, both `CMapDoodadDef`): 1.0, never 2.5
    //   < 0.5: a doodad on MCSH-shadowed ground: 0.5
    // Deviation: `min(I, 1)`. The reference bakes I into the SH unclamped (`0x71c4e0`), so a lit
    // entity commits ×2.5 there and ×1.0 here; lifting the cap takes sun-facing surfaces past 1.0.
    // `entity_shade::LIT_T` aims the lit ramp at 1.0 because of it: lift the cap and set `LIT_T`
    // back to 0.0 together.
    let inst_shade = select(f32((fade_tag >> 6u) & 0xffu) / 255.0, 0.0, interior_prop);
    let mat_shade = select(0.0, 1.0, m.sun_scale.x < 0.5);
    let shade_t = max(mat_shade, inst_shade);
    let mid_band = m.sun_scale.x >= 0.5 && m.sun_scale.x < 0.85;
    let intensity = min(select(mix(2.5, 0.5, shade_t), 1.0, mid_band), 1.0);
    // One `intensity` multiply covers every sun band (never I²); the c10 `.w` ambient does not
    // scale. `pack_model_core_rows` packs the same closed form.
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
    // Clamp the sum, never a term: the lobe's own ringing dips to −0.037·C near μ ≈ −0.53, and a
    // per-term clamp would erase it.
    let lit_doodad = clamp(sun_lobe, vec3<f32>(0.0), vec3<f32>(1.0));
    // WMO and clutter take the FFP N·L light with no terrain shade; the lobe needs the directional
    // light on (`light_sun.w`).
    let is_wmo = m.model_flags.x > 0.5;
    let is_interior = m.model_flags.z > 0.5;
    let use_doodad_shade = (wow_light.light_sun.w > 0.5) && !is_clutter && !is_wmo;
    let lit_exterior = select(lit_nl, lit_doodad, use_doodad_shade);
    // WMO interior groups (`groupFlags & 0x48 == 0`) by batch class (`tint.w`): INT (1) is unlit
    // `tex × MOCV`; TRANS (2) is the reference's two passes, lit × MOCV.a + unlit × (1 − MOCV.a),
    // as one lerp; EXT (0) is `lit_nl`. A WINDOW batch (MOMT 0x20) on the interior drawer lights
    // with ambient = diffuse = the Direct/Ambient midpoint, ambient +16/255 (`0x6d37e0`).
    var trans_a = 1.0;
#ifdef VERTEX_COLORS
    trans_a = in.color.a;
#endif
    let window_mid = 0.5 * (wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb);
    let lit_window = clamp(
        window_mid + vec3<f32>(16.0 / 255.0) + window_mid * ndotl,
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    let lit_int_base = select(lit_nl, lit_window, m.sidn.w > 0.5);
    var lit_wmo_interior = vec3<f32>(1.0);
    if (m.tint.w > 1.5) {
        lit_wmo_interior = mix(vec3<f32>(1.0), lit_int_base, trans_a);
    } else if (m.tint.w < 0.5) {
        lit_wmo_interior = lit_int_base;
    }
    // Interior M2 props (WMO MODD doodads): the reference commits the prop's MODD colour through a
    // fixed-axis diffuse lobe plus its group's MOLR lights as an order-2 SH probe, folded at spawn
    // by `lighting::prop_probe_coeffs` and evaluated here per fragment (the reference: per vertex).
    // Its soft wrap (≈ 0.088·C side-on) is the reference's response, not a max(N·L, 0).
#ifdef WOW_MERGED_SLOT
    // A merged blob bakes the slot per vertex; its tag carries only fog and alpha.
    let probe = 7u * in.merged_slot;
#else
    let probe = 7u * ((fade_tag >> 6u) & 0x1fffu);
#endif
    let lit_m2_interior = clamp(
        vec3<f32>(
            dot(wow_light.prop_probes[probe + 0u], n1)
                + dot(wow_light.prop_probes[probe + 3u], quad)
                + wow_light.prop_probes[probe + 6u].x * x2y2,
            dot(wow_light.prop_probes[probe + 1u], n1)
                + dot(wow_light.prop_probes[probe + 4u], quad)
                + wow_light.prop_probes[probe + 6u].y * x2y2,
            dot(wow_light.prop_probes[probe + 2u], n1)
                + dot(wow_light.prop_probes[probe + 5u], quad)
                + wow_light.prop_probes[probe + 6u].z * x2y2,
        ),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    let lit_interior = select(lit_m2_interior, lit_wmo_interior, is_wmo);
    // The authored-rig lane (`ShadeSel::Rig`, glue scenes): the scene's lights are folded into
    // probe slot 0 of the material's own buffer, which `lit_m2_interior` already evaluated (tag 0).
    let is_rig = m.sun_scale.x >= 1.5;
    let lit = select(select(lit_exterior, lit_interior, is_interior), lit_m2_interior, is_rig);
    // Gamma-space albedo: lighting runs on the authored byte values. `m.tint` plus its mat-anim
    // delta is the animated M2Color tint.
    let anim_tint = m.tint.rgb + wow_light.matanim[u32(m.anim_slots.y)].xyz;
    let albedo = base.rgb * anim_tint;
    // An unlit batch replaces the lit path: fullbright, with only the highlight added (below).
    let is_emissive = m.model_flags.w > 0.5;
    // SIDN night glow: the emissive × the night fraction (ramping 20:30→21:30, 06:00→07:00), a
    // GL_EMISSION term inside the clamped lit sum, on lit lanes only.
    var sidn_w = 1.0;
    if (is_interior && is_wmo) {
        if (m.tint.w > 1.5) {
            sidn_w = trans_a; // TRANS: weighted by its lit pass
        } else if (m.tint.w > 0.5) {
            sidn_w = 0.0; // INT: unlit, so no emission
        }
    }
    let sidn_e = m.sidn.rgb * (wow_light.grade.x * sidn_w);
    let point_diffuse = in.point_lit;

    // The hover/target highlight (tag bit 31): the scene's committed ambient
    // (`0x614576`-`0x6145bd`), added to the batch colour inside the final clamp, lit or unlit
    // (`c29`). Sampled live, where the reference holds the value sampled when the highlight
    // began: a unit's tag has no slot to hold a colour, and the ambient moves slowly.
    let highlight = select(vec3<f32>(0.0), wow_light.light_ambient.rgb, highlighted);
    // The FFP combine: the light sum (lit, point lights, emission) clamps first and the texture
    // modulates it, `tex × clamp(C·sum + emission)`, with C the GL_COLOR_MATERIAL colour (MOCV on
    // WMO, the body tint on M2). The WMO branch divides Bevy's MOCV fold back out, guarded; a dim
    // channel's product is ~0 either way.
    var lit_rgb: vec3<f32>;
#ifdef VERTEX_COLORS
    if (is_wmo) {
        let vc = in.color.rgb;
        let primary = clamp(
            vc * (lit + point_diffuse) + sidn_e + highlight,
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        let tex_rgb = base.rgb / max(vc, vec3<f32>(1.0 / 255.0));
        lit_rgb = tex_rgb * m.tint.rgb * primary;
        if (is_interior && m.tint.w > 0.5 && m.tint.w < 1.5) {
            // INT: the reference's interior pixel shader, `tex·MOCV.rgb·(1 + 4·MOCV.a)` with only
            // the framebuffer's final clamp.
            lit_rgb = clamp(
                tex_rgb * m.tint.rgb * vc * (1.0 + 4.0 * trans_a),
                vec3<f32>(0.0),
                vec3<f32>(1.0),
            );
        }
    } else {
        // An M2: the body tint multiplies the light terms as MOCV does above. A WMO surface has
        // no tint slot, so the WMO branch leaves it out.
        let primary = clamp(
            inst_tint * (lit + point_diffuse) + sidn_e + highlight,
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        lit_rgb = albedo * primary;
    }
#else
    // A WMO batch without MOCV lands here too; its tint slot is the identity slot 0.
    let primary = clamp(
        inst_tint * (lit + point_diffuse) + sidn_e + highlight,
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    lit_rgb = albedo * primary;
#endif
    // The unlit path: an M2's unlit program outputs `c28 + c29` (`0x70c663`-`0x70c693` fold the
    // tint·M2Color term into `c29` beside the highlight), so the texel modulates
    // `clamp(C·tint + highlight)`, C the M2Color. A WMO keeps the plain modulate.
    var unlit_rgb = albedo * inst_tint;
    if (!is_wmo) {
#ifdef VERTEX_COLORS
        // The constant M2Color rides the vertex colour, which Bevy folded into `base`.
        let unlit_c = in.color.rgb * anim_tint;
        let unlit_tex = base.rgb / max(in.color.rgb, vec3<f32>(1.0 / 255.0));
#else
        let unlit_c = anim_tint;
        let unlit_tex = base.rgb;
#endif
        let unlit_sum = unlit_c * inst_tint + highlight;
        unlit_rgb = unlit_tex * clamp(unlit_sum, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    var rgb = select(lit_rgb, unlit_rgb, is_emissive);
    // An M2 Mod or Mod2x batch draws the bare texel: the reference zeroes its tint·M2Color term and
    // forces the primary colour to the blend identity (`0x70c507`/`0x70c5b8`), so neither the
    // animated M2Color nor the body tint reaches it. Its alpha still does, through the lerp below.
    let is_mod = (u32(m.clutter_fade.z) & 128u) != 0u;
    let is_mod2x = (u32(m.clutter_fade.z) & 256u) != 0u;
    if ((is_mod || is_mod2x) && !is_wmo) {
        rgb = base.rgb;
    }

    // Linear fog by planar eye depth, as in terrain.wgsl. Per-batch colour policy (`clutter_fade.z`
    // bits 4-6, the M2 state setter `0x70baf0`): 0 scene, 1 black (additive), 2 white (Mod),
    // 3 grey (Mod2x), 4 unfogged (render flag 0x02). Tag bit 30, not the static `model_flags.z`,
    // selects the interior triple: WMO content by the per-group `[0xca7f00]` gate on the pushes
    // `0x6b5190`/`0x6b62e0`, an entity M2 by its node's classification (`0x71c110`, `[node+0xc]`).
    var fog_color = wow_light.fog_color;
    var fog_span = wow_light.fog_params.xy;
    if (interior_fogged) {
        fog_color = wow_light.wmo_fog_color;
        fog_span = wow_light.wmo_fog_params.xy;
    }
    let fog_policy = (u32(m.clutter_fade.z) >> 4u) & 7u;
    if (fog_color.w > 0.5 && fog_policy != 4u) {
        let eye_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        let denom = max(fog_span.y - fog_span.x, 0.001);
        let factor = clamp((fog_span.y - eye_z) / denom, 0.0, 1.0);
        var fog_rgb = fog_color.xyz;
        if (fog_policy == 1u) { fog_rgb = vec3<f32>(0.0); }
        else if (fog_policy == 2u) { fog_rgb = vec3<f32>(1.0); }
        else if (fog_policy == 3u) { fog_rgb = vec3<f32>(0.50196078); }
        rgb = mix(fog_rgb, rgb, factor);
    }


    var out: WowFragOut;
    // Output alpha is tex × fade. Opaque intent (`clutter_fade.z` bit 3) pins it to 1: a no-op
    // under correct pipeline state, and a guard for macOS/Metal with an extra camera, where an
    // opaque draw can bind a blending pipeline and show the BLP's garbage alpha.
    let opaque_intent = (u32(m.clutter_fade.z) & 8u) != 0u;
    // Additive (`clutter_fade.z` bit 2, the bit `specialize` keys the (ONE, ONE) blend on): the
    // alpha weight folds into the colour here, in gamma, as the reference weights its source.
    let is_additive = (u32(m.clutter_fade.z) & 4u) != 0u;
    var out_rgb = rgb;
    if (is_additive) {
        out_rgb = out_rgb * faded_alpha;
    }
    // Mod (bit 7) and Mod2x (bit 8) read no source alpha, so the fade rides the colour as in the
    // reference: texenv preset 5, `mix(prev.rgb, tex.rgb, prev.a)`, with the primary colour forced
    // to the blend identity (white, or 0.5 grey for Mod2x) and prev.a the instance alpha. The fog
    // above commutes with this because its white and grey are that identity.
    if (is_mod || is_mod2x) {
        let identity = select(vec3<f32>(1.0), vec3<f32>(0.5), is_mod2x);
        out_rgb = mix(identity, out_rgb, obj_fade);
    }
    // Raw gamma out: blending happens in gamma like the reference's byte framebuffer; the frame
    // decodes once, in the FFXGlow combine.
    out.color = vec4<f32>(out_rgb, select(faded_alpha, 1.0, opaque_intent));
    return out;
}
