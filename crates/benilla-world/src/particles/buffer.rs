//! The shared effect stream: one CPU vertex stream every dynamic-effect family writes each frame,
//! and the draw records slicing it. A writer calls [`EffectQuads::begin`], pushes world-space
//! vertices (whole quads in perimeter order, or a triangle list) and commits one draw for the
//! range; the render half rebases them camera-relative for upload (absolute f32 shears thin
//! geometry far from the origin) and sorts by [`EffectDraw::anchor`] view z plus the rung.

use std::ops::Range;

use benilla_formats::{ModelBlend, ParticleBlend};
use bevy::asset::AssetId;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;

/// One vertex of the lane: world-space in the stream, so instruments read world coordinates,
/// except on a [`EffectDrawSpec::cam_relative`] draw.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EffectVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    /// Raw authored gamma-space RGBA; alpha is the blend weight, never encoded.
    pub color: [f32; 4],
}

/// The lane's blends: [`ParticleBlend`]'s four plus the multiplicative pair of decals and rain.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EffectBlend {
    /// `(SRC_ALPHA, ONE)`, as premultiplied alpha after the shader's gamma `rgb·a` fold.
    Add,
    /// Standard alpha blending.
    Alpha,
    /// No blend, depth write on, drawn in the transparent bracket at the owner rung.
    Opaque,
    /// EGxBlend 1: [`EffectBlend::Opaque`]'s state plus an alpha test discarding below 224/255
    /// (`VANILLA_ALPHA_KEY_REF`, the mesh cutouts' ref; see [`ParticleBlend::AlphaKey`]).
    AlphaKey,
    /// `dst · lerp(1, src, α)`, bevy's `AlphaMode::Multiply` state: the blob shadow's
    /// `GL_DST_COLOR/GL_ZERO` with fade, and `ModelBlend::Mod` at α = 1.
    Multiply,
    /// `2·src·dst` as `(Dst, Src)`: rain's state.
    Mod2x,
}

impl From<ParticleBlend> for EffectBlend {
    fn from(blend: ParticleBlend) -> Self {
        match blend {
            ParticleBlend::Add => EffectBlend::Add,
            ParticleBlend::Alpha => EffectBlend::Alpha,
            ParticleBlend::AlphaKey => EffectBlend::AlphaKey,
            ParticleBlend::Opaque => EffectBlend::Opaque,
        }
    }
}

impl EffectBlend {
    /// The ground-fx mapping of a part's blend: `model_render`'s, with `additive` apart because M2
    /// modes 3 and 4 fold into [`ModelBlend::Blend`]. Deviation: `AlphaTest` maps to `Alpha`,
    /// because flat `Spells\` quads are blend batches and a cutout would bite a soft decal's fade;
    /// and the part draws unlit, as no lit ground-fx part is known.
    pub fn from_model(blend: ModelBlend, additive: bool) -> Self {
        if additive {
            // The material path's additive: the shader premultiplies `rgb·α` and returns α = 0.
            return EffectBlend::Add;
        }
        match blend {
            ModelBlend::Opaque => EffectBlend::Opaque,
            ModelBlend::AlphaTest | ModelBlend::Blend => EffectBlend::Alpha,
            ModelBlend::Mod => EffectBlend::Multiply,
            ModelBlend::Mod2x => EffectBlend::Mod2x,
        }
    }
}

/// How a draw's vertex range is indexed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectTopology {
    /// Whole quads: 4 perimeter-order corners each, indexed `[b, b+1, b+2, b, b+2, b+3]`.
    Quads,
    /// A plain triangle list: identity indices (the decal projector's fans, rain's streaks).
    Tris,
}

/// One draw's fog colour policy, one canonical row of the shader's params uniform.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EffectFog {
    /// No fog (params.x = 0): file flag 0x8, the decals and foam.
    Off,
    /// The scene fog (params.x = 1): Alpha and Opaque blends.
    Scene,
    /// Toward black (params.x = 2), for Add blends, from M2's per-blend fog table (`0x70baf0`).
    Black,
    /// Toward white (params.x = 3), the table's Mod policy: ground-fx `ModelBlend::Mod` decals.
    White,
    /// Toward grey 0.5 (params.x = 4), the table's Mod2x policy: ground-fx Mod2x decals.
    Grey,
    /// Rain's forced grey fog (params.y = 1, zw 70..75), its Mod2x distance fade (`0x59d350`).
    Rain,
}

impl EffectFog {
    /// The policy for one particle or ribbon def.
    pub fn for_blend(flags: u32, blend: ParticleBlend) -> Self {
        if flags & 0x8 != 0 {
            EffectFog::Off
        } else if matches!(blend, ParticleBlend::Add) {
            EffectFog::Black
        } else {
            EffectFog::Scene
        }
    }

    /// The `0x70baf0` fog policy a model material packs in `clutter_fade.z` bits 4..7.
    pub fn from_model_policy(policy: u32) -> Self {
        match policy {
            1 => EffectFog::Black,
            2 => EffectFog::White,
            3 => EffectFog::Grey,
            4 => EffectFog::Off,
            _ => EffectFog::Scene,
        }
    }

    /// The slot index into the render-world params uniform (one canonical `vec4` per policy).
    pub fn slot(self) -> u32 {
        match self {
            EffectFog::Off => 0,
            EffectFog::Scene => 1,
            EffectFog::Black => 2,
            EffectFog::White => 3,
            EffectFog::Grey => 4,
            EffectFog::Rain => 5,
        }
    }
}

/// Where one draw's light comes from. The reference builds an `M2Material` from each emitter
/// record, so a particle is lit like a submesh, by its model's own light node (`0x672a20`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum EffectLighting {
    /// Unlit: every family but M2 emitters, and the emitters that set the file's unlit bit 0x1.
    #[default]
    None,
    /// The scene's light, per view in the shader against world up (a booth's buffer overrides
    /// it): a lit emitter outdoors, mostly `World\` environment sheets.
    Scene,
    /// The model's own committed light in a WMO room, never the sun (`0x6a7300`'s interior leg);
    /// one normal per quad makes it one RGB, already folded into the vertex colours.
    Committed,
}

/// A per-emitter `WowLight` buffer read in place of the world's; the glue-scene booths set it.
#[derive(Component, Clone)]
pub struct EffectLightOverride(pub Buffer);

/// One draw of the lane: a vertex range, its texture, blend and fog, and its sort point.
pub struct EffectDraw {
    /// The main-world camera whose view draws this (the world's, or a booth's).
    pub cam: Entity,
    pub(crate) texture: AssetId<Image>,
    pub(crate) blend: EffectBlend,
    pub(crate) topology: EffectTopology,
    pub(crate) fog: EffectFog,
    /// The shader applies the scene's light: [`EffectLighting::Scene`] alone, a pipeline-key axis.
    pub lit: bool,
    /// The sort point: the emitter anchor, ribbon head node or decal centre.
    pub(crate) anchor: Vec3,
    /// The rung added to the view-space sort distance (`sky_order`: positive draws later).
    pub(crate) bias: f32,
    /// The rasterizer depth-bias constant, nonzero only for coplanar draws, which then skip the
    /// rebase for the world meshes' own `clip_from_world`, so the tie it settles is like for like.
    pub(crate) raster_bias: i32,
    pub(crate) raster_slope: f32,
    pub(crate) cam_relative: bool,
    pub(crate) no_depth_test: bool,
    /// Vertex range in [`EffectQuads::verts`] (a multiple of 4 for quads, 3 for tris).
    pub range: Range<u32>,
    /// The producing entity, the item's identity in the phase (`item.entity.1`).
    pub main_entity: Entity,
    /// [`EffectLightOverride`]'s buffer; `None` is the world's.
    pub(crate) light: Option<Buffer>,
    pub(crate) clip: Option<Vec4>,
}

/// The frame's stream: cleared by [`begin_effect_frame`], filled by the families, extracted.
#[derive(Resource)]
pub struct EffectQuads {
    pub verts: Vec<EffectVertex>,
    pub draws: Vec<EffectDraw>,
    /// Has [`begin_effect_frame`] run this frame? A commit before it would be erased, so
    /// [`Self::commit`] asserts it in debug builds: every writer needs
    /// `.after(begin_effect_frame)`. [`clear_effect_frame_flag`] resets it in `Last`.
    cleared_this_frame: bool,
}

impl Default for EffectQuads {
    fn default() -> Self {
        Self {
            verts: Vec::new(),
            draws: Vec::new(),
            // True, so a fixture with no clear can commit; the app's `Last` reset arms the check.
            cleared_this_frame: true,
        }
    }
}

/// Everything about one draw but its vertex range, as `commit_quads` and `commit_tris` take it.
pub struct EffectDrawSpec {
    pub cam: Entity,
    pub texture: AssetId<Image>,
    pub blend: EffectBlend,
    pub fog: EffectFog,
    /// The light the colour meets. The scene term is `clamp(ambient + diffuse·max(N·L, 0))` with
    /// N world up: the reference uploads one normal per draw, world +Z (`0x7b3fd0`).
    pub lighting: EffectLighting,
    pub anchor: Vec3,
    pub bias: f32,
    pub raster_bias: i32,
    /// The rasterizer depth-bias slope scale. The reference uses none: its one slope term
    /// (`0x59bf0a`, `glPolygonOffset` factor `0xc0800000` = −4.0) is in the GL arm, never built.
    pub raster_slope: f32,
    /// The producer wrote camera-relative vertices, so the upload rebase skips this draw: the
    /// snow flake's sub-centimetre geometry, which absolute f32 world coordinates cannot hold.
    /// [`Self::anchor`] stays absolute.
    pub cam_relative: bool,
    /// Depth compare `Always`: the weapon swing trail, whose callback turns the depth test off
    /// (EGxRs id `0x10` = 0, `0x6c686e`) so the swinger's shoulder never eats the arc.
    pub no_depth_test: bool,
    pub main_entity: Entity,
    pub light: Option<Buffer>,
    /// A target-pixel clip rect `(min.x, min.y, max.x, max.y)`; `None` is the whole target. UI
    /// model tiles share one atlas where the reference gives each `<Model>` its own viewport
    /// (`0x59c730`), so the fragment discards outside the pane's cell.
    pub clip: Option<Vec4>,
}

impl EffectQuads {
    /// Open a draw: remember where its vertices start.
    pub fn begin(&self) -> u32 {
        self.verts.len() as u32
    }

    /// Close a quad draw over everything pushed since `begin`; an empty range commits nothing.
    pub fn commit_quads(&mut self, start: u32, spec: EffectDrawSpec) {
        debug_assert_eq!((self.verts.len() as u32 - start) % 4, 0, "whole quads only");
        self.commit(start, EffectTopology::Quads, spec);
    }

    /// Close a triangle-list draw over everything pushed since `begin`.
    pub fn commit_tris(&mut self, start: u32, spec: EffectDrawSpec) {
        debug_assert_eq!(
            (self.verts.len() as u32 - start) % 3,
            0,
            "whole triangles only"
        );
        self.commit(start, EffectTopology::Tris, spec);
    }

    fn commit(&mut self, start: u32, topology: EffectTopology, spec: EffectDrawSpec) {
        debug_assert!(
            self.cleared_this_frame,
            "effect-stream write before `begin_effect_frame`: this draw is about to be erased. \
             The producing system needs `.after(crate::particles::buffer::begin_effect_frame)` \
             (see `EffectQuads::cleared_this_frame`)."
        );
        let end = self.verts.len() as u32;
        if end > start {
            self.draws.push(EffectDraw {
                cam: spec.cam,
                texture: spec.texture,
                blend: spec.blend,
                topology,
                fog: spec.fog,
                lit: spec.lighting == EffectLighting::Scene,
                anchor: spec.anchor,
                bias: spec.bias,
                raster_bias: spec.raster_bias,
                raster_slope: spec.raster_slope,
                cam_relative: spec.cam_relative,
                no_depth_test: spec.no_depth_test,
                range: start..end,
                main_entity: spec.main_entity,
                light: spec.light,
                clip: spec.clip,
            });
        }
    }
}

/// Clear the stream for a new frame; every family's writer runs after it.
pub fn begin_effect_frame(mut quads: ResMut<EffectQuads>) {
    quads.verts.clear();
    quads.draws.clear();
    quads.cleared_this_frame = true;
}

/// Reset [`EffectQuads::cleared_this_frame`] in `Last`, after extraction.
pub fn clear_effect_frame_flag(mut quads: ResMut<EffectQuads>) {
    quads.cleared_this_frame = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M2 modes 2 to 4 fold into `ModelBlend::Blend`; flat `Spells\` ground quads are mode 4.
    #[test]
    fn additive_wins_over_the_folded_blend_enum() {
        for blend in [
            ModelBlend::Opaque,
            ModelBlend::AlphaTest,
            ModelBlend::Blend,
            ModelBlend::Mod,
            ModelBlend::Mod2x,
        ] {
            assert_eq!(
                EffectBlend::from_model(blend, true),
                EffectBlend::Add,
                "{blend:?} + additive must reach the pure-add state, not {:?}",
                EffectBlend::from_model(blend, false),
            );
        }
    }

    /// `model_render`'s mapping, with `AlphaTest` folded to `Alpha`.
    #[test]
    fn non_additive_keeps_the_material_paths_law() {
        assert_eq!(
            EffectBlend::from_model(ModelBlend::Opaque, false),
            EffectBlend::Opaque
        );
        assert_eq!(
            EffectBlend::from_model(ModelBlend::AlphaTest, false),
            EffectBlend::Alpha
        );
        assert_eq!(
            EffectBlend::from_model(ModelBlend::Blend, false),
            EffectBlend::Alpha
        );
        assert_eq!(
            EffectBlend::from_model(ModelBlend::Mod, false),
            EffectBlend::Multiply
        );
        assert_eq!(
            EffectBlend::from_model(ModelBlend::Mod2x, false),
            EffectBlend::Mod2x
        );
    }

    /// A writer without `.after(begin_effect_frame)` runs first when the clear carries a
    /// dependency it lacks; the graph is `ParticlePlugin`'s clear and `EntitiesPlugin`'s beam.
    mod write_order {
        use super::*;

        #[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
        struct Place;

        fn joint_palette() {}
        fn finalize_rigs() {}
        fn face_billboards() {}

        fn writer(mut quads: ResMut<EffectQuads>) {
            let start = quads.begin();
            for _ in 0..4 {
                quads.verts.push(EffectVertex {
                    pos: [0.0; 3],
                    uv: [0.0; 2],
                    color: [1.0; 4],
                });
            }
            quads.commit_quads(
                start,
                EffectDrawSpec {
                    cam: Entity::PLACEHOLDER,
                    texture: AssetId::default(),
                    blend: EffectBlend::Add,
                    fog: EffectFog::Off,
                    lighting: EffectLighting::None,
                    anchor: Vec3::ZERO,
                    bias: 0.0,
                    raster_bias: 0,
                    raster_slope: 0.0,
                    cam_relative: false,
                    no_depth_test: false,
                    main_entity: Entity::PLACEHOLDER,
                    light: None,
                    clip: None,
                },
            );
        }

        /// `edged`: whether the writer declares `.after(begin_effect_frame)`.
        fn run(edged: bool) -> App {
            let mut app = App::new();
            app.init_resource::<EffectQuads>();
            app.add_systems(
                PostUpdate,
                (joint_palette, finalize_rigs, face_billboards)
                    .chain()
                    .in_set(Place),
            );
            let w = writer
                .in_set(Place)
                .after(joint_palette)
                .after(finalize_rigs);
            app.add_systems(
                PostUpdate,
                if edged {
                    w.after(begin_effect_frame)
                } else {
                    w
                },
            );
            app.add_systems(
                PostUpdate,
                begin_effect_frame
                    .in_set(Place)
                    .after(joint_palette)
                    .after(finalize_rigs)
                    // The dependency the writer lacks.
                    .after(face_billboards),
            );
            app.add_systems(Last, clear_effect_frame_flag);
            // Two frames: the check only bites after the first `Last` reset.
            app.update();
            app.update();
            app
        }

        /// With the edge, the writer's draw survives the frame and reaches extract.
        #[test]
        fn the_edge_keeps_the_draw() {
            let app = run(true);
            let quads = app.world().resource::<EffectQuads>();
            assert_eq!(quads.draws.len(), 1, "the draw must survive to extract");
            assert_eq!(quads.verts.len(), 4);
        }

        /// Without it, the write precedes the clear and the debug assert fires.
        #[test]
        #[should_panic(expected = "effect-stream write before `begin_effect_frame`")]
        fn without_the_edge_the_tripwire_names_it() {
            let _ = run(false);
        }
    }
}

/// Draw a batch of quads or triangles in the effect lane: the writer for everything but
/// particles, with the lane's defaults for what it does not set.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldEffectDraw<'w> {
    quads: ResMut<'w, EffectQuads>,
}

impl<'w> WorldEffectDraw<'w> {
    /// Open a batch drawn through `cam` with `texture`; it commits when closed, if not empty.
    pub fn batch(&mut self, cam: Entity, texture: AssetId<Image>) -> EffectBatch<'_> {
        let start = self.quads.begin();
        EffectBatch {
            start,
            spec: EffectDrawSpec {
                cam,
                texture,
                blend: EffectBlend::Alpha,
                fog: EffectFog::Off,
                lighting: EffectLighting::None,
                anchor: Vec3::ZERO,
                bias: 0.0,
                raster_bias: 0,
                raster_slope: 0.0,
                cam_relative: false,
                no_depth_test: false,
                main_entity: Entity::PLACEHOLDER,
                light: None,
                clip: None,
            },
            quads: &mut self.quads,
        }
    }
}

/// One open batch, pushed to and closed by its caller.
pub struct EffectBatch<'a> {
    quads: &'a mut EffectQuads,
    start: u32,
    spec: EffectDrawSpec,
}

impl EffectBatch<'_> {
    /// Additive: glows, rings, beams.
    pub fn additive(mut self) -> Self {
        self.spec.blend = EffectBlend::Add;
        self
    }

    /// `SRC_ALPHA / INV_SRC_ALPHA`, the default, for a reference alpha state (the swing trail's
    /// EGxRs `0x07 = 2`).
    pub fn alpha(mut self) -> Self {
        self.spec.blend = EffectBlend::Alpha;
        self
    }

    /// Multiply into what is drawn: the blob shadow.
    pub fn multiply(mut self) -> Self {
        self.spec.blend = EffectBlend::Multiply;
        self
    }

    /// An explicit blend, for a lane whose authored data carries one.
    pub fn blend(mut self, blend: EffectBlend) -> Self {
        self.spec.blend = blend;
        self
    }

    /// An explicit fog policy; the default is [`EffectFog::Off`].
    pub fn fog(mut self, fog: EffectFog) -> Self {
        self.spec.fog = fog;
        self
    }

    /// The world point this batch sorts by.
    pub fn anchored(mut self, at: Vec3) -> Self {
        self.spec.anchor = at;
        self
    }

    /// The draw-order rung: sort bias and raster depth-bias constant (`crate::sky_order::Rung`).
    pub fn rung(mut self, sort: f32, raster: i32) -> Self {
        self.spec.bias = sort;
        self.spec.raster_bias = raster;
        self
    }

    /// Occluded by nothing; see [`EffectDrawSpec::no_depth_test`].
    pub fn over_everything(mut self) -> Self {
        self.spec.no_depth_test = true;
        self
    }

    /// The entity this draw belongs to, for the render world's per-item bookkeeping.
    pub fn owner(mut self, entity: Entity) -> Self {
        self.spec.main_entity = entity;
        self
    }

    /// Push vertices built elsewhere (the decal lanes project once and cache).
    pub fn vertices(&mut self, verts: &[EffectVertex]) {
        self.quads.verts.extend_from_slice(verts);
    }

    /// Push vertices from an iterator.
    pub fn extend(&mut self, verts: impl IntoIterator<Item = EffectVertex>) {
        self.quads.verts.extend(verts);
    }

    /// The vertex sink, for a producer that writes in place.
    pub fn verts_mut(&mut self) -> &mut Vec<EffectVertex> {
        &mut self.quads.verts
    }

    /// Close the batch as a triangle list.
    pub fn tris(self) {
        self.quads.commit_tris(self.start, self.spec);
    }

    /// Close it as quads.
    pub fn quads(self) {
        self.quads.commit_quads(self.start, self.spec);
    }
}
