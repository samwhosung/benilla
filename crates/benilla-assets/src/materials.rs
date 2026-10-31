//! The render materials: the four `ExtendedMaterial`s the world is drawn with (terrain splat,
//! M2/WMO model, WDL far band, liquid surface) and the WGSL each one binds.
//!
//! The WGSL is embedded (`embedded://benilla_assets/shaders/…`), not served: a relative path would
//! resolve against the host binary's `AssetPlugin::file_path` and render nothing elsewhere.

use bevy::image::Image;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MeshPipelineKey,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer, ColorWrites,
    CompareFunction, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

/// Compile the four WGSL files into the binary under `embedded://benilla_assets/shaders/…`. Call
/// after Bevy's `AssetPlugin`, whose registry this fills; [`crate::register_asset_loaders`] does.
pub fn register_shaders(app: &mut App) {
    bevy::asset::embedded_asset!(app, "shaders/terrain.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/wow_model.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/wdl.wgsl");
    bevy::asset::embedded_asset!(app, "shaders/liquid.wgsl");
}

/// The WDL far-band shader's source, for the tests that live beside the renderer (`wdl.rs`).
pub const WDL_WGSL: &str = include_str!("shaders/wdl.wgsl");

/// Alpha-test reference for blend mode 1 (`Blend_AlphaKey`): 224 in the reference's
/// per-blend-mode table at `0x85ad20`, `{0, 224, 1, 1, 1, 1, 1, 0, 0, 0, 0}`. Must stay in sync
/// with `VANILLA_ALPHA_KEY` in `shaders/wow_model.wgsl`.
pub const VANILLA_ALPHA_KEY_REF: f32 = 224.0 / 255.0;

/// `StandardMaterial` plus the per-tile layer-blend extension.
pub type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExtension>;

/// World models (doodads, WMOs, creatures, GameObjects) lit in gamma space like terrain, not by
/// PBR; `StandardMaterial` carries only the texture, alpha mode and culling.
pub type WowModelMaterial = ExtendedMaterial<StandardMaterial, WowModelExt>;

/// Pipeline-specialization key for [`WowModelExt`]: the depth and blend states Bevy's `AlphaMode`
/// cannot express. `fade` (`model_flags.y`) is the M2 doodad fade twin and `clutter`
/// (`clutter_fade.w`) ground clutter; both fade opacity with depth-write on, as the reference does.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WowModelKey {
    fade: bool,
    clutter: bool,
    /// Additive glow card (`clutter_fade.z` bit 2).
    additive: bool,
    /// M2 render flag 0x10, no depth write (`clutter_fade.z` bit 0).
    no_depth_write: bool,
    /// M2 render flag 0x08, no depth test (`clutter_fade.z` bit 1).
    no_depth_test: bool,
    /// The multiply blends, Mod and Mod2x (`clutter_fade.z` bits 7/8).
    modulate: bool,
    modulate2x: bool,
    /// The depth-prime twin (`clutter_fade.z` bit 9, `model_render::zfill_material`).
    zfill: bool,
    /// Far side of the water plane (`clutter_fade.z` bit 11).
    far_side: bool,
    /// The WMO-skybox lane (`clutter_fade.z` bit 13, `model_render::SKY_DEPTH_MARKER`), a key axis
    /// because only this model's depth is pinned.
    sky_depth: bool,
    // The WMO batch order is deliberately not a key axis (a pipeline per batch index stalls a
    // city's first sight): the file-order layering (`0x6b4f10`/`0x6b5190`) rides `sun_scale.y`.
}

impl From<&WowModelExt> for WowModelKey {
    fn from(e: &WowModelExt) -> Self {
        let markers = e.clutter_fade.z as u32;
        Self {
            fade: e.model_flags.y > 0.5,
            clutter: e.clutter_fade.w > 0.5,
            additive: markers & 4 != 0,
            no_depth_write: markers & 1 != 0,
            no_depth_test: markers & 2 != 0,
            modulate: markers & 0x80 != 0,
            modulate2x: markers & 0x100 != 0,
            zfill: markers & 0x200 != 0,
            far_side: markers & 0x800 != 0,
            sky_depth: markers & 0x2000 != 0,
        }
    }
}

/// The model material's extension. Every `Vec4` uniform packs into binding 100, one buffer entry,
/// to stay under Metal's 16-buffer vertex-stage cap (Bevy 0.18 ignores `visibility` on uniforms);
/// `ModelParams` in `wow_model.wgsl` must match the field order.
#[derive(Asset, AsBindGroup, Clone, TypePath)]
#[bind_group_data(WowModelKey)]
pub struct WowModelExt {
    /// Clutter fade: `x`, `y` = full-opacity and fully-faded view depth (yd; the ~70 yd horizon),
    /// `w` = clutter, all zero off clutter; `z` = the batch marker bits.
    #[uniform(100)]
    pub clutter_fade: Vec4,
    /// `x` = WMO, `y` = the fade blend twin, `z` = interior, `w` = unlit fullbright.
    #[uniform(100)]
    pub model_flags: Vec4,
    /// `x` = the terrain-shade selector, `y` = the WMO batch order (the clip-z layering nudge),
    /// `zw` = the UV-animation offset.
    #[uniform(100)]
    pub sun_scale: Vec4,
    /// `xyz` = the M2Color tint of a batch whose colour track animates, identity otherwise; `w` =
    /// the WMO interior batch class (`0x6b5190`): `0` exterior, `1` INT, `2` TRANS.
    #[uniform(100)]
    pub tint: Vec4,
    /// WMO glass, `0` on M2: `xyz` = the MOMT SIDN night emissive (ramp `0x6b4090`), `w` = the MOMT
    /// WINDOW flag, the brighter interior light (`0x6d37e0`, ambient +16/255).
    #[uniform(100)]
    pub sidn: Vec4,
    /// Rows of the shared light buffer's `matanim` region, `0` = identity: `x` = UV scroll, `y` =
    /// tint, `z` = the texture-transform affine, `w` = the UI tile's cell clip.
    #[uniform(100)]
    pub anim_slots: Vec4,
    /// The second texture stage: `x` = bound (1) or not (0), `y` = its combine preset 0-5
    /// (`benilla_formats::TextureStage::combine`); `z` = the stage-0 combine preset plus one
    /// (`benilla_formats::RenderSubmesh::combine`), 0 for a batch that has none; `w` unused.
    #[uniform(100)]
    pub stage1: Vec4,
    /// Stage 1's texture, sampled at the generated environment coordinate when `stage1.x` is set;
    /// `None` binds the fallback image, unread.
    #[texture(101, visibility(fragment))]
    #[sampler(102, visibility(fragment))]
    pub stage1_texture: Option<Handle<Image>>,
    /// The shared global light (`lighting::global_light`), updated in place once a frame; the
    /// vertex stage reads its point-light table, since Bevy's clusterable lights are fragment-only.
    #[storage(90, read_only, buffer, visibility(vertex, fragment))]
    pub light_buf: Buffer,
}

impl MaterialExtension for WowModelExt {
    /// Bevy's mesh vertex plus the point-light term, per vertex like the reference's FFP.
    fn vertex_shader() -> ShaderRef {
        "embedded://benilla_assets/shaders/wow_model.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_assets/shaders/wow_model.wgsl".into()
    }

    /// The reference writes depth for every M2 batch, transparent ones too, and tests `LEQUAL`,
    /// unless render flag 0x10 (no write) or 0x08 (no test) clears it (`0x70c190`).
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // `WOW_RIG_SKIN` skins from the shared buffer's palette. The base layout lacks our joint
        // attributes: rebuild it with Bevy's conditionals (mesh.rs, locations 0-5) plus 10/11.
        if layout.0.contains(crate::ATTRIBUTE_WOW_JOINT_INDEX) {
            descriptor.vertex.shader_defs.push("WOW_RIG_SKIN".into());
            let mut attrs = Vec::with_capacity(7);
            for (attr, loc) in [
                (Mesh::ATTRIBUTE_POSITION, 0),
                (Mesh::ATTRIBUTE_NORMAL, 1),
                (Mesh::ATTRIBUTE_UV_0, 2),
                (Mesh::ATTRIBUTE_UV_1, 3),
                (Mesh::ATTRIBUTE_TANGENT, 4),
                (Mesh::ATTRIBUTE_COLOR, 5),
            ] {
                if layout.0.contains(attr) {
                    attrs.push(attr.at_shader_location(loc));
                }
            }
            attrs.push(crate::ATTRIBUTE_WOW_JOINT_INDEX.at_shader_location(10));
            attrs.push(crate::ATTRIBUTE_WOW_JOINT_WEIGHT.at_shader_location(11));
            descriptor.vertex.buffers = vec![layout.0.get_layout(&attrs)?];
        }
        // A merged blob computes the doodad fade per vertex from its fade sphere; same rebuild,
        // and merged blobs never skin.
        if layout.0.contains(crate::ATTRIBUTE_WOW_FADE_SPHERE) {
            descriptor.vertex.shader_defs.push("WOW_MERGED_FADE".into());
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("WOW_MERGED_FADE".into());
            }
            let mut attrs = Vec::with_capacity(6);
            for (attr, loc) in [
                (Mesh::ATTRIBUTE_POSITION, 0),
                (Mesh::ATTRIBUTE_NORMAL, 1),
                (Mesh::ATTRIBUTE_UV_0, 2),
                (Mesh::ATTRIBUTE_UV_1, 3),
                (Mesh::ATTRIBUTE_TANGENT, 4),
                (Mesh::ATTRIBUTE_COLOR, 5),
            ] {
                if layout.0.contains(attr) {
                    attrs.push(attr.at_shader_location(loc));
                }
            }
            attrs.push(crate::ATTRIBUTE_WOW_FADE_SPHERE.at_shader_location(12));
            // An interior-prop blob's baked SH-probe slot replaces the per-entity MeshTag payload.
            if layout.0.contains(crate::ATTRIBUTE_WOW_MERGED_SLOT) {
                descriptor.vertex.shader_defs.push("WOW_MERGED_SLOT".into());
                if let Some(fragment) = descriptor.fragment.as_mut() {
                    fragment.shader_defs.push("WOW_MERGED_SLOT".into());
                }
                attrs.push(crate::ATTRIBUTE_WOW_MERGED_SLOT.at_shader_location(13));
            }
            descriptor.vertex.buffers = vec![layout.0.get_layout(&attrs)?];
        }
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.depth_write_enabled = !key.bind_group_data.no_depth_write;
            if key.bind_group_data.no_depth_test {
                ds.depth_compare = CompareFunction::Always;
            }
        }
        // A sort bias is not a raster bias: the base `StandardMaterial::specialize` also packs it
        // into `depth_stencil.bias.constant`, where the far side's −4e4 would clip coplanar layers.
        if key.bind_group_data.far_side {
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.bias.constant = 0;
            }
        }
        // The skybox pins clip z to 0, reverse-Z far, in the vertex stage; its bias is sort-only.
        if key.bind_group_data.sky_depth {
            descriptor.vertex.shader_defs.push("WOW_SKY_DEPTH".into());
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("WOW_SKY_DEPTH".into());
            }
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.bias.constant = 0;
            }
        }
        if key.bind_group_data.fade && !key.bind_group_data.sky_depth {
            // A fading model's near cards occlude its far ones; the sky never writes depth.
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.depth_write_enabled = true;
            }
        }
        if key.bind_group_data.clutter {
            // Clutter blends in the alpha-mask pass, as the reference does, so the ~70 yd ramp
            // fades opacity; the shader keeps the 128/255 discard.
            if let Some(target) = descriptor
                .fragment
                .as_mut()
                .and_then(|f| f.targets.get_mut(0))
                .and_then(|t| t.as_mut())
            {
                target.blend = Some(BlendState::ALPHA_BLENDING);
            }
        }
        if key.bind_group_data.additive {
            // A pure (ONE, ONE) add: the shader weights by alpha in gamma space, where a `SrcAlpha`
            // factor or `AlphaMode::Add` would weight after linearising and fatten soft edges.
            if let Some(target) = descriptor
                .fragment
                .as_mut()
                .and_then(|f| f.targets.get_mut(0))
                .and_then(|t| t.as_mut())
            {
                target.blend = Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::One,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent {
                        src_factor: BlendFactor::Zero,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                });
            }
        }
        if key.bind_group_data.zfill {
            // The reference's `M2UseZFill` pre-pass (`0x707f7d`-`0x708072`): depth only, so a
            // translucent model shows one blended layer. The colour is computed and masked, not
            // skipped by an early return, which naga's MSL backend miscompiles.
            if let Some(frag) = descriptor.fragment.as_mut() {
                if let Some(target) = frag.targets.get_mut(0).and_then(|t| t.as_mut()) {
                    target.blend = None;
                    target.write_mask = ColorWrites::empty();
                }
            }
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.depth_write_enabled = true;
            }
        }
        // The waterline clip (`benilla_world::straddle`) keys on the blend pass: no new pipelines.
        if key
            .mesh_key
            .intersection(MeshPipelineKey::BLEND_RESERVED_BITS)
            == MeshPipelineKey::BLEND_ALPHA
        {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("WOW_WATER_CLIP".into());
            }
        }
        let key = &key.bind_group_data;
        if key.modulate || key.modulate2x {
            // `0x70c190`: Mod (M2 mode 5, WMO 4) is `DST_COLOR/ZERO`, Mod2x (M2 6, WMO 5)
            // `DST_COLOR/SRC_COLOR`; on a gamma framebuffer this is the reference's byte math.
            if let Some(target) = descriptor
                .fragment
                .as_mut()
                .and_then(|f| f.targets.get_mut(0))
                .and_then(|t| t.as_mut())
            {
                target.blend = Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::Dst,
                        dst_factor: if key.modulate2x {
                            BlendFactor::Src
                        } else {
                            BlendFactor::Zero
                        },
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent {
                        src_factor: BlendFactor::Zero,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                });
            }
        }
        Ok(())
    }
}

/// Distant low-detail terrain (WDL), the horizon hills beyond the streamed tiles: opaque white
/// geometry under the scene fog colour at the hull pass's own start 0 and end 1.0.
pub type WdlMaterial = ExtendedMaterial<StandardMaterial, WdlExt>;

/// WDL has no per-material input: its whole colour is the scene fog, off the shared global light.
#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct WdlExt {
    /// The shared global light, rows 4/5 only (scene fog colour, farclip wall).
    #[storage(90, read_only, buffer)]
    pub light_buf: Buffer,
}

impl MaterialExtension for WdlExt {
    fn vertex_shader() -> ShaderRef {
        "embedded://benilla_assets/shaders/wdl.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_assets/shaders/wdl.wgsl".into()
    }
}

/// Liquid surfaces, one `liquid.wgsl` arm per reference liquid renderer: ADT MCLQ (the
/// `ocean0_s.bls` combine), WMO exterior and interior water, and magma and slime.
pub type LiquidMaterial = ExtendedMaterial<StandardMaterial, LiquidExt>;

/// Liquid inputs: the animated frames and the per-material lanes; all light comes off the shared
/// global light. The `Vec4` uniforms pack into binding 102, in the WGSL struct's field order.
#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct LiquidExt {
    /// The kind's animated frames (`lake_a`, `fast_a`, `ocean_h`): RGB near-black, alpha ripple.
    #[texture(100, dimension = "2d_array", visibility(fragment))]
    #[sampler(101, visibility(fragment))]
    pub frames: Handle<Image>,
    /// Which lanes of the shared light this surface reads:
    /// - `x` = fullbright (magma, slime): the sheet is the opaque body.
    /// - `y` = ocean: rows 15/16 (`Light.dbc` IntBand 14/15), else rows 13/14 (IntBand 16/17).
    /// - `z` = a WMO interior group's liquid, fogged with the interior block (rows 18/19): the
    ///   reference gates its fog (`0x6b6323`-`0x6b6342`) on the same `[0xca7f00]` as the group's
    ///   geometry (`0x6b51d9`/`0x6b51ea`).
    /// - `w` = the sun-sheen shininess (`lighting::WATER_SHININESS`).
    #[uniform(102)]
    pub kind: Vec4,
    /// `x` = the renderer (`liquid::surface::LiquidPath`): `0` ADT MCLQ, `1` WMO exterior, `2` WMO
    /// interior.
    #[uniform(102)]
    pub path: Vec4,
    /// `y` = frame count, `z` = scroll flag, `w` = clock enable.
    #[uniform(102)]
    pub anim: Vec4,
    /// The shared global light, read in both stages (the vertex stage evaluates the sun sheen).
    #[storage(90, read_only, buffer, visibility(vertex, fragment))]
    pub light_buf: Buffer,
}

impl MaterialExtension for LiquidExt {
    fn vertex_shader() -> ShaderRef {
        "embedded://benilla_assets/shaders/liquid.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_assets/shaders/liquid.wgsl".into()
    }

    /// `sky_order::WATER_BIAS` (−2e4) is a sort rung, kept out of the rasterizer: as a depth-bias
    /// constant it would move the waterline, by an amount that doubles at every float exponent
    /// boundary, so neighbouring triangles' shorelines would disagree.
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.bias.constant = 0;
        }
        Ok(())
    }
}

/// Per-tile terrain splat, one material per ADT tile: each merged-mesh vertex carries its chunk's
/// four layer indices (`COLOR`) and alpha layer (`UV1.x`), so a tile is one draw.
///
/// One repeating sampler (105) serves every array, to stay under Metal's 16 fragment samplers;
/// `terrain.wgsl` insets the alpha UVs by half a texel so they do not wrap at a chunk edge.
#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct TerrainExtension {
    #[texture(100, dimension = "2d_array", visibility(fragment))]
    #[sampler(105, visibility(fragment))]
    pub layer_array: Handle<Image>,
    #[texture(104, dimension = "2d_array", visibility(fragment))]
    pub alpha_array: Handle<Image>,
    /// Per-chunk MCSH shadow maps (`R` = shadowed); `UV1.y` is the layer, `-1` for none.
    #[texture(110, dimension = "2d_array", visibility(fragment))]
    pub shadow_array: Handle<Image>,

    // Binding 106's uniforms, in the field order of the WGSL `TerrainParams`.
    /// `x` = texture tiling factor (repeats per chunk); other lanes unused.
    #[uniform(106)]
    pub params: Vec4,

    /// The shared global light, rows 0-5 (light, fog, farclip).
    #[storage(90, read_only, buffer)]
    pub light_buf: Buffer,
}

impl MaterialExtension for TerrainExtension {
    // The sun specular is per vertex (the reference's light flush `0x59c820`).
    fn vertex_shader() -> ShaderRef {
        "embedded://benilla_assets/shaders/terrain.wgsl".into()
    }
    fn fragment_shader() -> ShaderRef {
        "embedded://benilla_assets/shaders/terrain.wgsl".into()
    }
}

#[cfg(test)]
mod tests {
    /// The sky depth law for the WMO skybox, the one sky element on the model lane.
    #[test]
    fn the_sky_lane_pins_the_far_depth_at_the_vertex() {
        let src = include_str!("shaders/wow_model.wgsl");
        // The pin, matched behind its ifdef: ungated, every model draw sits at the far plane.
        let pin = src
            .find("#ifdef WOW_SKY_DEPTH")
            .expect("the sky-depth branch is gone");
        let branch = &src[pin..src[pin..].find("#endif").map_or(src.len(), |e| pin + e)];
        assert!(
            branch.contains("out.position.z = 0.0;"),
            "the model lane's sky branch no longer pins the far depth at the vertex — a WMO \
             skybox's shell radius is deciding occlusion again (benilla_world::sky_order, \"The \
             depth law\")"
        );
        assert!(
            !src.contains("@builtin(frag_depth)"),
            "the model lane writes a fragment depth again — the sky pin is the vertex stage's, \
             and a fragment write costs every draw on this lane its early-Z"
        );
    }

    /// The reference lights in the vertex stage: `Model2.bls` evaluates the SH lobe and writes the
    /// colour to `oD0`, and WMO and clutter run FFP T&L, so the colour is clamped at each vertex
    /// and interpolated. The light lanes live in `vertex_light`, the vertex stage writes its result
    /// to the interpolated `primary`, and the fragment only modulates the texel by it.
    #[test]
    fn the_model_light_is_evaluated_per_vertex() {
        let src = include_str!("shaders/wow_model.wgsl");
        let at = |needle: &str| {
            src.find(needle)
                .unwrap_or_else(|| panic!("`{needle}` is gone"))
        };
        let light = &src[at("fn vertex_light(")..at("@vertex")];
        let vertex = &src[at("@vertex")..at("@fragment")];
        let fragment = &src[at("@fragment")..];
        for (lane, term) in [
            ("the exterior SH lobe", "wow_light.sh_c10_r.w + sun_dc.x"),
            ("the FFP N·L", "let ndotl = max(dot(n_lit, L), 0.0);"),
            (
                "the interior probe",
                "dot(wow_light.prop_probes[probe + 0u], n1)",
            ),
        ] {
            assert!(light.contains(term), "{lane} left `vertex_light`");
        }
        assert!(
            vertex.contains("out.primary = vertex_light(out.world_normal, point_lit, vcol,"),
            "the vertex stage no longer lights its vertex"
        );
        assert!(
            src.contains("    @location(8) primary: vec3<f32>,\n"),
            "the lit colour must be interpolated across the triangle, never flat"
        );
        for term in [
            "sh_c10",
            "sh_c13",
            "prop_probes",
            "light_sun",
            "light_diffuse",
            "point_light_sum",
            "vertex_light(",
        ] {
            assert!(
                !fragment.contains(term),
                "the fragment evaluates `{term}` again — the reference lights per vertex"
            );
        }
        assert!(
            fragment.contains("var lit_rgb = base.rgb * in.primary;"),
            "the fragment modulates the texel by the interpolated lit colour"
        );
    }

    /// The vertex or model colour is the lit material, inside the one clamp, and the texel
    /// multiplies the clamped result. The FFP takes it as GL_COLOR_MATERIAL on ambient and diffuse
    /// (`0x59cfec`, `0x59cff3`-`0x59cffd`), so a WMO lights `clamp(MOCV·sum + emission)`.
    /// `Model2.bls` takes it as `c28`, `[model+0x1a0]` × the batch's M2Color (`0x70c60a`-`0x70c654`,
    /// written at `0x70c712`), and writes `c28·(E, 1) + c29` to the saturating `oD0`, clamping E
    /// first only with a point light bound. The install tests below read that law in every lit
    /// program and run two of them against the M2 lane.
    #[test]
    fn the_material_colour_is_lit_inside_the_one_clamp() {
        let src = include_str!("shaders/wow_model.wgsl");
        let at = |needle: &str| {
            src.find(needle)
                .unwrap_or_else(|| panic!("`{needle}` is gone"))
        };
        let light = &src[at("fn vertex_light(")..at("@vertex")];
        let fragment = &src[at("@fragment")..];
        for (why, line) in [
            (
                "the FFP sum is never clamped on its own",
                "let sum_nl = wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * ndotl;",
            ),
            (
                "the window ambient saturates as its own byte add, never the sum",
                "let sum_window = min(window_mid + vec3<f32>(16.0 / 255.0), vec3<f32>(1.0))\n            \
                 + window_mid * ndotl;",
            ),
            (
                "a WMO's MOCV multiplies the light sum inside the one clamp",
                "return clamp(vcol.rgb * sum_wmo + sidn_e + highlight, vec3<f32>(0.0), vec3<f32>(1.0));",
            ),
            (
                "an M2's `c28` multiplies E inside the one clamp",
                "return clamp(c * e + highlight, vec3<f32>(0.0), vec3<f32>(1.0));",
            ),
            (
                "E clamps first only with a point light bound",
                "if (point_lit.w > 0.5) {\n        e = clamp(e, vec3<f32>(0.0), vec3<f32>(1.0));\n    }",
            ),
        ] {
            assert!(light.contains(line), "{why}: `{line}` is gone");
        }
        assert!(
            !light.contains("clamp(sun_lobe"),
            "the SH lobe clamps before `c28` multiplies it again"
        );
        assert!(
            src.contains("vo.color = vec4<f32>(1.0, 1.0, 1.0, in.color.a);"),
            "Bevy folds the vertex colour into the texel again, outside the clamp"
        );
        assert!(
            fragment
                .contains("lit_rgb = base.rgb * m.tint.rgb * mix(vcol.rgb, in.primary, vcol.a);"),
            "a TRANS batch is its clamped lit pass by MOCV.a over the unlit `tex × MOCV`"
        );
    }

    /// A ground-clutter tuft's vertex colour, `0xffc0c0c0` on baked shadow and white elsewhere
    /// (`0x6b260e`/`0x6b2615`), is the FFP's lit material: the detail-doodad pass commits the
    /// chunk's lights (`0x681656`) and draws with colour material on ambient and diffuse
    /// (`0x59cfec`), so the grey multiplies the light sum inside the one clamp.
    #[test]
    fn a_clutter_tufts_grey_is_lit_inside_the_clamp() {
        let src = include_str!("shaders/wow_model.wgsl");
        let at = |needle: &str| {
            src.find(needle)
                .unwrap_or_else(|| panic!("`{needle}` is gone"))
        };
        let light = &src[at("fn vertex_light(")..at("@vertex")];
        let fragment = &src[at("@fragment")..];
        assert!(
            light.contains(
                "return clamp(c * (sum_nl + point_lit.rgb) + highlight, vec3<f32>(0.0), vec3<f32>(1.0));"
            ),
            "the tuft's grey left the clamp of its light sum"
        );
        assert!(
            light.contains("let c = vcol.rgb * anim_tint * body_tint(fade_tag);"),
            "the tuft's material colour is its vertex colour"
        );
        assert!(
            !fragment.contains("albedo * in.primary"),
            "the fragment multiplies the grey outside the clamp again"
        );
    }

    /// The install's `Shaders\Vertex\Model2.bls`, or `None` (a skip) without an install.
    fn model2_bls() -> Option<Vec<u8>> {
        let data = benilla_formats::wow_data_or_skip!(None);
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let bls = chain
            .read_file("Shaders\\Vertex\\Model2.bls")
            .expect("read Model2.bls");
        assert_eq!(
            &bls[..8],
            b"SVXG\x03\x00\x01\x00",
            "a vertex shader file, version 0x10003"
        );
        Some(bls)
    }

    /// One level's 1800 programs in vertex shader id order: the level table holds a file offset
    /// per level at `0xc` (`0x5951e6`), and each record is two empty parameter lists, the format
    /// tag (2 Direct3D, 0 ARB), the code size and the code padded to 4 bytes (`0x598320`).
    fn model2_level(bls: &[u8], level: usize) -> Vec<&[u8]> {
        let word = |at: usize| u32::from_le_bytes(bls[at..at + 4].try_into().unwrap()) as usize;
        let tag = if level == 1 { 2 } else { 0 };
        let mut at = word(0xc + 4 * level);
        let mut out = Vec::with_capacity(1800);
        while out.len() < 1800 {
            assert_eq!(
                (word(at), word(at + 4), word(at + 8)),
                (0, 0, tag),
                "level {level} record {} at {at:#x}",
                out.len()
            );
            let size = word(at + 12);
            out.push(&bls[at + 16..at + 16 + size]);
            at += 16 + size.next_multiple_of(4);
        }
        out
    }

    /// The reference fact `vertex_light` follows, read from the install's `Model2.bls` at both
    /// levels, Direct3D (1, the default device, `vs_2_0`) and ARB (2): every lit program of the
    /// vertex-lit set (digit L 1, P 0; `0x71de50` builds the id, `0x707cf6` adds the bound light
    /// count as its last digit) ends `c28·(E, 1) + c29` into the primary colour, and clamps E
    /// with `min`/`max` exactly when it binds a point light.
    #[test]
    fn model2_clamps_the_light_sum_only_with_a_point_light_bound() {
        let Some(bls) = model2_bls() else { return };
        let lit = |id: usize| id < 900 && (id / 90) & 1 == 1;

        // Level 1: `vs_2_0` tokens. An instruction's length is bits 24-27; a register's type is
        // bits 28-30 and 11-12, its number bits 0-10, a destination's write mask bits 16-19.
        let reg = |t: u32| (((t >> 28) & 7) | ((t >> 8) & 0x18), t & 0x7ff);
        for (id, code) in model2_level(&bls, 1).into_iter().enumerate() {
            if !lit(id) {
                continue;
            }
            let t: Vec<u32> = code
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_le_bytes(*c))
                .collect();
            assert_eq!(t[0], 0xfffe_0200, "program {id} is vs_2_0");
            let mut ops: Vec<(u32, &[u32])> = Vec::new();
            let mut i = 1;
            while t[i] & 0xffff != 0xffff {
                let op = t[i] & 0xffff;
                let len = if op == 0xfffe {
                    (t[i] >> 16) & 0x7fff
                } else {
                    (t[i] >> 24) & 0xf
                } as usize;
                if op != 0xfffe {
                    ops.push((op, &t[i + 1..i + 1 + len]));
                }
                i += 1 + len;
            }
            // `mad oD0, c28, E, c29`: oD0 is type 5 register 0, c29 type 2 register 29.
            let out = ops
                .iter()
                .position(|(op, a)| *op == 4 && reg(a[0]) == (5, 0))
                .unwrap_or_else(|| panic!("program {id} writes no oD0 by mad"));
            assert_eq!(reg(ops[out].1[3]), (2, 29), "program {id} adds c29");
            // A clamp of E is `min` (10) and `max` (11) into a temp's xyz.
            let clamps = |opcode: u32| {
                ops[..out]
                    .iter()
                    .any(|(op, a)| *op == opcode && reg(a[0]).0 == 0 && (a[0] >> 16) & 0xf == 7)
            };
            assert_eq!(
                clamps(10) && clamps(11),
                id % 5 > 0,
                "Direct3D program {id}, {} point lights bound",
                id % 5
            );
        }

        // Level 2: ARB text.
        let mut seen = 0;
        for (id, code) in model2_level(&bls, 2).into_iter().enumerate() {
            if !lit(id) {
                continue;
            }
            seen += 1;
            let text = std::str::from_utf8(code).expect("ARB text");
            let lines: Vec<&str> = text.lines().map(str::trim).collect();
            let out = lines
                .iter()
                .find(|l| l.contains("result.color.front.primary"))
                .unwrap_or_else(|| panic!("program {id} writes no primary colour"));
            assert!(
                out.starts_with("MAD result.color.front.primary,") && out.ends_with("c28[1];"),
                "program {id} ends `{out}`"
            );
            let clamps = |op: &str| {
                lines
                    .iter()
                    .any(|l| l.starts_with(op) && l.contains(".xyz, c0."))
            };
            assert_eq!(
                clamps("MIN R") && clamps("MAX R"),
                id % 5 > 0,
                "ARB program {id}, {} point lights bound",
                id % 5
            );
        }
        assert_eq!(seen, 450, "the vertex-lit set's lit programs");
    }

    /// Runs one ARB vertex program on one vertex: `local` is `program.local[]`, `attribs` binds
    /// `vertex.position` and `vertex.normal`. Returns the primary colour, which the GL clamps to
    /// [0, 1] before it interpolates. Covers the instructions of the vertex-lit programs.
    fn run_arb(text: &str, local: &[[f32; 4]; 96], attribs: &[(&str, [f32; 4])]) -> [f32; 3] {
        use std::collections::HashMap;
        enum Param {
            Literal([f32; 4]),
            Local(usize),
        }
        let mut params: HashMap<&str, Param> = HashMap::new();
        let mut regs: HashMap<String, [f32; 4]> = HashMap::new();
        let split_mask = |s: &str| -> (String, String) {
            match s.rsplit_once('.') {
                Some((name, m)) if !m.is_empty() && m.chars().all(|c| "xyzw".contains(c)) => {
                    (name.to_string(), m.to_string())
                }
                _ => (s.to_string(), "xyzw".to_string()),
            }
        };
        for line in text.lines().map(str::trim) {
            let line = line.trim_end_matches(';');
            if line.is_empty()
                || line.starts_with("!!")
                || line == "END"
                || line.starts_with("TEMP")
            {
                continue;
            }
            if let Some(rest) = line.strip_prefix("PARAM ") {
                let (name, value) = rest.split_once(" = ").unwrap();
                let name = name.split('[').next().unwrap();
                let value = value.trim_matches(|c| c == '{' || c == '}' || c == ' ');
                let param = match value.strip_prefix("program.local[") {
                    Some(range) => Param::Local(range.split("..").next().unwrap().parse().unwrap()),
                    None => {
                        let v: Vec<f32> = value
                            .split(',')
                            .map(|x| x.trim().parse().unwrap())
                            .collect();
                        Param::Literal([v[0], v[1], v[2], v[3]])
                    }
                };
                params.insert(name, param);
                continue;
            }
            if let Some(rest) = line.strip_prefix("ATTRIB ") {
                let (name, binding) = rest.split_once(" = ").unwrap();
                let v = attribs
                    .iter()
                    .find(|(b, _)| *b == binding)
                    .map_or([0.0; 4], |a| a.1);
                regs.insert(name.to_string(), v);
                continue;
            }
            let (op, operands) = line.split_once(char::is_whitespace).unwrap();
            let operands: Vec<&str> = operands.trim().split(", ").collect();
            let read = |s: &str| -> [f32; 4] {
                let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
                let (name, swz) = split_mask(s);
                let v = if let Some((base, idx)) = name.split_once('[') {
                    let idx: usize = idx.trim_end_matches(']').parse().unwrap();
                    match params[base] {
                        Param::Local(b) => local[b + idx],
                        Param::Literal(_) => panic!("an indexed literal `{s}`"),
                    }
                } else if let Some(r) = regs.get(&name) {
                    *r
                } else {
                    match params[name.as_str()] {
                        Param::Local(b) => local[b],
                        Param::Literal(v) => v,
                    }
                };
                let swz: Vec<usize> = swz.chars().map(|c| "xyzw".find(c).unwrap()).collect();
                std::array::from_fn(|k| {
                    let x = v[swz[k.min(swz.len() - 1)]];
                    if neg {
                        -x
                    } else {
                        x
                    }
                })
            };
            let src: Vec<[f32; 4]> = operands[1..].iter().map(|s| read(s)).collect();
            let each = |f: &dyn Fn(usize) -> f32| -> [f32; 4] { std::array::from_fn(f) };
            let r = match op {
                "MOV" => src[0],
                "ADD" => each(&|k| src[0][k] + src[1][k]),
                "MUL" => each(&|k| src[0][k] * src[1][k]),
                "MAD" => each(&|k| src[0][k] * src[1][k] + src[2][k]),
                "MIN" => each(&|k| src[0][k].min(src[1][k])),
                "MAX" => each(&|k| src[0][k].max(src[1][k])),
                "DP3" => [(0..3).map(|k| src[0][k] * src[1][k]).sum::<f32>(); 4],
                "DP4" => [(0..4).map(|k| src[0][k] * src[1][k]).sum::<f32>(); 4],
                "RSQ" => [1.0 / src[0][0].abs().sqrt(); 4],
                "RCP" => [1.0 / src[0][0]; 4],
                _ => panic!("an instruction the vertex-lit programs do not use: `{line}`"),
            };
            let (dst, mask) = split_mask(operands[0]);
            let slot = regs.entry(dst).or_insert([0.0; 4]);
            for c in mask.chars() {
                let k = "xyzw".find(c).unwrap();
                slot[k] = r[k];
            }
        }
        let c = regs["result.color.front.primary"];
        [c[0], c[1], c[2]].map(|x| x.clamp(0.0, 1.0))
    }

    /// `vertex_light`'s M2 lane against the reference itself: Model2.bls programs 135 (lit, no
    /// point light) and 136 (one bound) run on the same constants as a mirror of the lane. The
    /// WGSL checks pin the shader to the mirror's shape: the SH basis, the point light's falloff,
    /// E clamped only with a light bound, and `c28·E + c29` clamped once.
    #[test]
    fn vertex_light_runs_model2_programs_135_and_136() {
        let src = include_str!("shaders/wow_model.wgsl");
        for line in [
            "let n_lit = wow_normalize(world_normal);",
            "let quad = vec4<f32>(n_lit.x * n_lit.y, n_lit.y * n_lit.z, n_lit.z * n_lit.z, n_lit.x * n_lit.z);",
            "let n1 = vec4<f32>(n_lit, 1.0);",
            "let x2y2 = n_lit.x * n_lit.x - n_lit.y * n_lit.y;",
            "let atten = 1.0 / (0.7 * d + 0.03 * d * d);",
            "let nl = max(dot(N, to_light / max(d, 1e-4)), 0.0);",
            "sum += vec4<f32>(wow_light.points[2u * sel[s] + 1u].rgb * (atten * nl), 1.0);",
            "var e = select(e_exterior, e_probe, is_interior || is_rig) + point_lit.rgb;",
            "if (point_lit.w > 0.5) {\n        e = clamp(e, vec3<f32>(0.0), vec3<f32>(1.0));\n    }",
            "return clamp(c * e + highlight, vec3<f32>(0.0), vec3<f32>(1.0));",
        ] {
            assert!(src.contains(line), "the M2 lane left its mirror: `{line}` is gone");
        }
        let Some(bls) = model2_bls() else { return };
        let arb = model2_level(&bls, 2);

        // The lit material colour `c28`, the highlight `c29`, the committed SH rows c10-c16
        // (the sum passes 1 on the lit side) and one point light's colour and position.
        let c28 = [0.5f32, 0.8, 1.0];
        let c29 = [0.1f32, 0.0, 0.05];
        let sh: [[f32; 4]; 7] = [
            [0.30, 0.55, 0.20, 0.70],
            [0.25, 0.45, 0.15, 0.55],
            [0.10, 0.30, 0.05, 0.40],
            [0.12, -0.05, 0.08, 0.10],
            [0.09, -0.04, 0.06, 0.08],
            [0.05, -0.02, 0.03, 0.04],
            [0.07, 0.05, 0.03, 0.0],
        ];
        let (light_pos, light_rgb) = ([2.0f32, 3.0, 1.0], [1.5f32, 1.0, 0.5]);
        let mut local = [[0.0f32; 4]; 96];
        local[2] = [1.0, 0.0, 0.0, 0.0]; // c2-c5: the projection, identity
        local[3] = [0.0, 1.0, 0.0, 0.0];
        local[4] = [0.0, 0.0, 1.0, 0.0];
        local[5] = [0.0, 0.0, 0.0, 1.0];
        local[10..17].copy_from_slice(&sh);
        local[17] = [light_rgb[0], light_rgb[1], light_rgb[2], 0.0]; // c17: colour
        local[21] = [light_pos[0], light_pos[1], light_pos[2], 1.0]; // c21: position
        local[25] = [0.0; 4]; // c25-c27: constant, linear, quadratic falloff
        local[26] = [0.7, 0.0, 0.0, 0.0];
        local[27] = [0.03, 0.0, 0.0, 0.0];
        local[28] = [c28[0], c28[1], c28[2], 1.0];
        local[29] = [c29[0], c29[1], c29[2], 0.0];
        local[31] = [1.0, 0.0, 0.0, 0.0]; // c31-c33: the bone, identity
        local[32] = [0.0, 1.0, 0.0, 0.0];
        local[33] = [0.0, 0.0, 1.0, 0.0];

        /// `vertex_light`'s M2 lane, step for step, on committed SH rows.
        fn lane(
            sh: &[[f32; 4]; 7],
            n: [f32; 3],
            p: [f32; 3],
            light: Option<([f32; 3], [f32; 3])>,
            c: [f32; 3],
            highlight: [f32; 3],
        ) -> [f32; 3] {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let n = n.map(|x| x / len);
            let quad = [n[0] * n[1], n[1] * n[2], n[2] * n[2], n[0] * n[2]];
            let x2y2 = n[0] * n[0] - n[1] * n[1];
            let mut e: [f32; 3] = std::array::from_fn(|ch| {
                let c10 = sh[ch];
                let c13 = sh[3 + ch];
                c10[3]
                    + (c10[0] * n[0] + c10[1] * n[1] + c10[2] * n[2])
                    + (0..4).map(|k| c13[k] * quad[k]).sum::<f32>()
                    + sh[6][ch] * x2y2
            });
            if let Some((pos, rgb)) = light {
                let to: [f32; 3] = std::array::from_fn(|k| pos[k] - p[k]);
                let d = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
                let atten = 1.0 / (0.7 * d + 0.03 * d * d);
                let nl = ((n[0] * to[0] + n[1] * to[1] + n[2] * to[2]) / d.max(1e-4)).max(0.0);
                e = std::array::from_fn(|k| (e[k] + rgb[k] * atten * nl).clamp(0.0, 1.0));
            }
            std::array::from_fn(|k| (c[k] * e[k] + highlight[k]).clamp(0.0, 1.0))
        }

        let p = [0.5f32, -0.2, 0.3];
        let mut order_shows = false;
        for n in [
            [0.3f32, 0.8, 0.5],
            [0.9, 0.6, 1.2], // unnormalized: the program normalizes the bone-space normal
            [-0.4, 0.2, -0.9],
            [0.0, 0.0, 1.0],
        ] {
            let attribs = [
                ("vertex.position", [p[0], p[1], p[2], 1.0]),
                ("vertex.normal", [n[0], n[1], n[2], 0.0]),
            ];
            for (id, light) in [(135usize, None), (136, Some((light_pos, light_rgb)))] {
                let text = std::str::from_utf8(arb[id]).expect("ARB text");
                let want = run_arb(text, &local, &attribs);
                let got = lane(&sh, n, p, light, c28, c29);
                for k in 0..3 {
                    assert!(
                        (got[k] - want[k]).abs() < 1e-5,
                        "program {id}, n {n:?}, ch{k}: the lane gives {} where Model2.bls gives {}",
                        got[k],
                        want[k]
                    );
                }
                // The constants reach the clamps: with no light, clamping E before `c28` would
                // have given another colour somewhere.
                if light.is_none() {
                    let clamped_first =
                        lane(&sh, n, p, Some(([100.0, 0.0, 0.0], [0.0; 3])), c28, c29);
                    order_shows |= (0..3).any(|k| (clamped_first[k] - want[k]).abs() > 1e-3);
                }
            }
        }
        assert!(
            order_shows,
            "the sample never puts E past 1 under a c28 below 1"
        );
    }

    /// A second texture stage samples `Model2.bls`'s generated coordinate: the program for a stage
    /// whose coordinate lookup is above 2 reflects the view-space position about the normal
    /// (`DP3`, `MUL 2`, `MAD −`), normalizes it (`RSQ`) and maps x, y by `MAD ·0.5 + 0.5`.
    #[test]
    fn stage1_samples_the_model2_environment_coordinate() {
        let src = include_str!("shaders/wow_model.wgsl");
        assert!(
            src.contains("let refl = normalize(p_view - 2.0 * dot(p_view, n_view) * n_view);")
                && src.contains("return refl.xy * 0.5 + vec2<f32>(0.5, 0.5);"),
            "the environment coordinate left `Model2.bls`'s reflection"
        );
        assert!(
            src.contains(
                "out.env_uv = wow_env_uv(view_rot * p_cam, normalize(view_rot * out.world_normal));"
            ),
            "stage 1 no longer generates its coordinate in view space"
        );
        assert!(
            src.contains(
                "tex1 = textureSampleBias(stage1_texture, stage1_sampler, in.env_uv, view.mip_bias);"
            ),
            "stage 1 no longer samples its texture at the generated coordinate"
        );

        /// The ARB program's arithmetic, step for step.
        fn env_uv(p: [f32; 3], n: [f32; 3]) -> [f32; 2] {
            let pn = p[0] * n[0] + p[1] * n[1] + p[2] * n[2];
            let r: [f32; 3] = std::array::from_fn(|k| p[k] - 2.0 * pn * n[k]);
            let inv = 1.0 / (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
            [r[0] * inv * 0.5 + 0.5, r[1] * inv * 0.5 + 0.5]
        }
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
        let h = std::f32::consts::FRAC_1_SQRT_2;
        // Facing the eye straight on: R = (0, 0, 5), the texture's centre.
        assert!(close(env_uv([0.0, 0.0, -5.0], [0.0, 0.0, 1.0]), [0.5, 0.5]));
        // Off-axis on a facing normal: R = (3, 0, 4), u = 0.5 + 0.3.
        assert!(close(env_uv([3.0, 0.0, -4.0], [0.0, 0.0, 1.0]), [0.8, 0.5]));
        // A 45° normal turns the view ray to +x: the rim, u = 1.
        assert!(close(env_uv([0.0, 0.0, -4.0], [h, 0.0, h]), [1.0, 0.5]));
        // An upward normal above the eye: R = (0, −2, −2), v = 0.5 − 0.5·√½.
        assert!(close(
            env_uv([0.0, 2.0, -2.0], [0.0, 1.0, 0.0]),
            [0.5, 0.5 - 0.5 * h]
        ));
    }

    /// Stage 1 combines by the gx preset tables of the default device, Direct3D (`gxApi`,
    /// `0x63a230`): colour `0x80a25c` (4, 4, 13, 7, 5, 12 as `D3DTOP`) and alpha `0x80a274` (3, 4,
    /// 3, 7, 5, 3), args TEXTURE and CURRENT. The OpenGL records of `0x85c250` differ only in the
    /// weights of presets 2 and 5. Every stock two-texture unit is preset 1, colour and alpha
    /// modulated.
    #[test]
    fn stage1_combines_by_the_gx_presets() {
        let src = include_str!("shaders/wow_model.wgsl");
        for (line, why) in [
            (
                "var c = prev * tex.rgb;",
                "presets 0 and 1 modulate the colour",
            ),
            (
                "c = mix(prev, tex.rgb, tex.a);",
                "preset 2, BLENDTEXTUREALPHA, blends by the texel's alpha",
            ),
            ("c = prev + tex.rgb;", "preset 3 adds"),
            ("c = 2.0 * prev * tex.rgb;", "preset 4 modulates twice"),
            (
                "c = mix(prev, tex.rgb, diffuse_a);",
                "preset 5, BLENDDIFFUSEALPHA, blends by the diffuse alpha",
            ),
            (
                "return clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));",
                "a stage saturates",
            ),
            ("a = prev * tex;", "preset 1 modulates the alpha"),
            ("a = prev + tex;", "preset 3 adds the alpha"),
            ("a = 2.0 * prev * tex;", "preset 4 doubles the alpha"),
            (
                "rgb = stage1_rgb(stage1_preset, rgb, tex1, obj_fade);",
                "stage 1 combines with the lit stage-0 result, preset 5 by the diffuse alpha",
            ),
            (
                "base_color.a = stage_alpha(stage1_preset, base_color.a, tex1.a);",
                "stage 1's alpha joins the alpha the cutout reads",
            ),
        ] {
            assert!(src.contains(line), "{why}: `{line}` is gone");
        }
        // Stage 1 combines after the light and before the fog, as the stages precede the fog.
        let combine = src.find("rgb = stage1_rgb(").expect("the combine");
        let fog = src
            .find("var fog_color = wow_light.fog_color;")
            .expect("the fog");
        let lit = src
            .find("var rgb = select(lit_rgb, unlit_rgb, is_emissive);")
            .expect("lit");
        assert!(
            lit < combine && combine < fog,
            "stage 1 sits between the light and the fog"
        );

        /// Preset 1 on one texel: the stock units' combine.
        fn preset1(prev: [f32; 4], tex: [f32; 4]) -> [f32; 4] {
            std::array::from_fn(|k| (prev[k] * tex[k]).clamp(0.0, 1.0))
        }
        let out = preset1([0.8, 0.6, 0.4, 1.0], [0.5, 0.25, 1.0, 0.5]);
        assert_eq!(out, [0.4, 0.15, 0.4, 0.5], "colour and alpha both modulate");
    }

    /// A fade twin blends by the alpha chain on the instance alpha, not the texel's: stage 0
    /// combines the diffuse alpha (`0x70c73a`, c28.w) with the texel by its preset's alpha op
    /// (`0x80a274` = 3, 4, 3, 7, 5, 3 as `D3DTOP`, args TEXTURE and CURRENT, `ALPHAARG` never set),
    /// so an opaque batch's preset 0 (`0x811f8c`) blends by the fade alone, and the 0x101 models'
    /// preset 1 by texel × fade.
    #[test]
    fn a_fade_twin_blends_by_the_stage_zero_alpha_op() {
        let src = include_str!("shaders/wow_model.wgsl");
        for (line, why) in [
            (
                "faded_alpha = stage_alpha(u32(m.stage1.z) - 1u, obj_fade, tex0_alpha);",
                "the blended alpha is stage 0's alpha op on the fade and the texel",
            ),
            (
                "faded_alpha = stage_alpha(stage1_preset, faded_alpha, tex1.a);",
                "stage 1 follows on the faded alpha",
            ),
            (
                "let tex0_alpha = base_color.a;",
                "stage 0's texel alpha is read before stage 1 joins it",
            ),
            (
                "fn stage_alpha(preset: u32, prev: f32, tex: f32) -> f32 {\n    var a = prev;",
                "an alpha op that selects ARG2 passes the previous alpha, the fade, whole",
            ),
            (
                "out.color = vec4<f32>(out_rgb, select(faded_alpha, 1.0, opaque_intent));",
                "the output alpha is the chain's",
            ),
        ] {
            assert!(src.contains(line), "{why}: `{line}` is gone");
        }
        // The old output multiplied the texel into every twin, whatever its combine.
        assert!(
            !src.contains("let faded_alpha = base_color.a * obj_fade;"),
            "the twin's alpha is the texel times the fade again, preset 0 included"
        );

        /// `stage_alpha`, step for step.
        fn stage_alpha(preset: u32, prev: f32, tex: f32) -> f32 {
            match preset {
                1 => prev * tex,
                3 => prev + tex,
                4 => 2.0 * prev * tex,
                _ => prev,
            }
            .clamp(0.0, 1.0)
        }
        // A texel at alpha 0 (the Wind Rider's `WyvernSkin02` batch) fading at 0.5.
        let (fade, texel) = (0.5, 0.0);
        assert_eq!(stage_alpha(0, fade, texel), 0.5, "preset 0: the fade alone");
        assert_eq!(stage_alpha(1, fade, texel), 0.0, "preset 1: texel × fade");
        // Preset 0's alpha does not move with the texel; preset 1's does.
        for t in [0.0, 0.25, 1.0] {
            assert_eq!(stage_alpha(0, fade, t), fade);
            assert_eq!(stage_alpha(1, fade, t), fade * t);
        }
        // The rest of the alpha ops: 2 and 5 select ARG2, 3 adds, 4 doubles the product.
        assert_eq!(stage_alpha(2, 0.5, 0.25), 0.5);
        assert_eq!(stage_alpha(5, 0.5, 0.25), 0.5);
        assert_eq!(stage_alpha(3, 0.5, 0.25), 0.75);
        assert_eq!(stage_alpha(4, 0.5, 0.25), 0.25);
    }

    /// The WGSL carries the reference's swatch row arithmetic, and a Rust mirror of it reproduces
    /// the row fill `0x68a830` on `Light.dbc` id 4, map 0, t = 1440, ocean `LightIntBand` 14 to 15.
    #[test]
    fn liquid_swatch_reproduces_the_reference_row_ramp() {
        let src = include_str!("shaders/liquid.wgsl");
        // The row accumulator `c0 + floor(i*(c1 - c0)/64)`, not a lerp to the deep endpoint.
        assert!(
            src.contains("let row = c0 + floor(i * (c1 - c0) / 64.0);"),
            "the swatch stopped building its rows the way `0x68a830` does: a plain lerp to the \
             deep endpoint runs a 64th of a ramp that does not exist"
        );
        // The ocean-only tail: floor(0.9*byte) on the last row, alpha forced opaque.
        assert!(
            src.contains("if ocean && i >= 63.0 {")
                && src.contains("vec4<f32>(floor(row.rgb * 0.9), 255.0)"),
            "the ocean's last-row darkening is gone — ~80% of the world's ocean vertices sample \
             that row, so this is the open sea's colour"
        );
        // Linear sampling across the two rows V falls between, at texel `V*64 - 0.5`.
        assert!(
            src.contains("let t = clamp(v * 64.0 - 0.5, 0.0, 63.0);"),
            "the swatch stopped sampling as an 8x64 LINEAR/CLAMP texture — the ocean darkening \
             would step instead of ramping across the final 1/64 of V"
        );

        /// `row(i)` for one channel, `swatch_row`'s byte arithmetic.
        fn row(c0: i32, c1: i32, i: i32) -> i32 {
            c0 + (i * (c1 - c0)).div_euclid(64)
        }
        /// The ocean tail's colour half.
        fn tail(b: i32) -> i32 {
            (f64::from(b) * 0.9).floor() as i32
        }

        // Ocean shallow (sub-14) 0x00457b63 and deep (sub-15) 0x000b2228, as R/G/B.
        let (shallow, deep) = ([69, 123, 99], [11, 34, 40]);
        let at = |i| [0, 1, 2].map(|k| row(shallow[k], deep[k], i));

        // Row 63 is one short of the endpoint on G: the ramp never arrives.
        assert_eq!(at(63), [11, 35, 40], "row 63 before the ocean tail");
        assert_ne!(
            at(63)[1],
            deep[1],
            "the ramp must NOT reach the deep endpoint"
        );
        assert_eq!(
            at(62),
            [12, 36, 41],
            "row 62, the other half of the last blend"
        );
        assert_eq!(
            at(63).map(tail),
            [9, 31, 36],
            "row 63 after the ocean tail (HSV V *= 0.9, which is floor(0.9*byte) per channel)"
        );
        // The tail and the shortfall are deep-end only.
        assert_eq!(at(0), shallow, "row 0 is the shallow endpoint verbatim");
    }
}
