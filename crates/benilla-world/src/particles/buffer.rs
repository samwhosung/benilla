//! The **shared effect stream** — the one CPU vertex stream every dynamic-effect family writes
//! into each frame (particles, ribbons, and — since 0733 — the decal family, water foam, and
//! precipitation), and the draw records that slice it (decisions 0732 P1/P2).
//!
//! Before this, every effect owned a `Mesh` asset rewritten per frame: ~145 mesh `Modified`
//! events/frame at the LBRS pin, each a full free+realloc in Bevy's mesh allocator (no partial
//! update exists — `allocator.rs:650`), and together they held the `AssetChanged<Mesh3d>`
//! short-circuit open so every material type's specialization probe ran over its whole
//! population every frame. One shared buffer + directly-constructed `Transparent3d` items
//! (the bevy_ui_render shape) converts the whole family from population-priced to
//! change-priced: one `write_buffer` per frame, zero mesh assets, zero material assets.
//!
//! The write protocol: a system calls [`EffectQuads::begin`], pushes **world-space** vertices —
//! whole quads (4 corners in perimeter order, closed by the `[0,1,2, 0,2,3]` pattern) or a
//! triangle list — then commits one draw for the range ([`EffectQuads::commit_quads`] /
//! [`EffectQuads::commit_tris`]). The render half rebases every draw's vertices against its
//! target view's camera position before upload (absolute coordinates through the view
//! transform shear thin geometry apart far from the origin; the precip module learned this
//! empirically at ~9000 yd), builds the frame's index stream in sorted-item order, and merges
//! sort-adjacent draws that share (pipeline, texture, light, fog) into single draw calls
//! (0732 P2). The sort key is [`EffectDraw::anchor`] view-z + [`EffectDraw::bias`] — the ladder
//! rungs (owner-last 0719/0721, the decal biases, foam's water tie-break) moved from material
//! `depth_bias` into the item key (`sky_order`'s sign law); the rasterizer half of the old
//! material `depth_bias` lives on as [`EffectDraw::raster_bias`] (the coplanar decals need it).
//!
//! The rebase is a *late* subtraction, which is fine until a producer's geometry is smaller than
//! an f32 ULP at the position it writes — the rounding has already happened by then. The snow
//! flake reached that (millimetre quads at ~5600-yd coordinates), so a draw may declare
//! [`EffectDrawSpec::cam_relative`] and do the subtraction itself.

use std::ops::Range;

use benilla_formats::{ModelBlend, ParticleBlend};
use bevy::asset::AssetId;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;

/// One vertex of the shared lane. **World-space** position in the stream; the render-world
/// prepare pass rebases it camera-relative before upload, so instruments reading the
/// stream (depth probe, depth dump) always see world coordinates — with one declared exception,
/// [`EffectDrawSpec::cam_relative`], whose producer has already done the subtraction because its
/// geometry is too small to survive being written in absolute-world f32 at all.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EffectVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    /// RAW authored gamma-space RGBA (the GAMMA LANE invariant) — alpha is the blend
    /// weight, never encoded.
    pub color: [f32; 4],
}

/// The lane's blend variants — a superset of the file-format enums it serves:
/// [`ParticleBlend`]'s four, plus the multiplicative pair the decal family and rain need.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EffectBlend {
    /// `(SRC_ALPHA, ONE)` via premultiplied-alpha + the shader's gamma `rgb·a` fold.
    Add,
    /// Standard alpha blending.
    Alpha,
    /// No blend, depth-write ON — drawn in the transparent bracket at the owner rung.
    Opaque,
    /// [`EffectBlend::Opaque`]'s state plus the fixed-function **alpha test** — the fragment is
    /// discarded below `224/255` ([`benilla_assets`]'s `VANILLA_ALPHA_KEY_REF`, the same ref the
    /// mesh path's cutouts take). EGxBlend 1, the debris/chip family; the byte chain that pins
    /// blend-off + z-write-on + that ref is on [`ParticleBlend::AlphaKey`].
    AlphaKey,
    /// `dst · lerp(1, src, α)` — bevy's `AlphaMode::Multiply` state (`(Dst, 1−srcα)` + shader
    /// premultiply, bevy_pbr mesh.rs:2486): the blob shadow's `GL_DST_COLOR/GL_ZERO`-with-fade,
    /// and `ModelBlend::Mod` at α = 1.
    Multiply,
    /// `2·src·dst` — `(Dst, Src)`, 0528's factors; rain's verified state (rf-weather-render).
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
    /// The ground-fx mapping from a part's authored blend — `model_render.rs`'s law with the
    /// lane's two named approximations: `AlphaTest` folds to `Alpha` (**deliberately not**
    /// [`EffectBlend::AlphaKey`], which the particle side now takes — the groundscan census says
    /// flat `Spells\` quads are blend batches, and a cutout ref on a soft-edged decal would bite
    /// its fade), and the part renders unlit (spell fx are; a lit ground quad would differ — none
    /// observed).
    ///
    /// `additive` is a SECOND, non-optional input because [`ModelBlend`] cannot express additive:
    /// M2 blend modes 3/4 fold into its `Blend` variant (see its own doc, "Alpha-blended /
    /// additive"), and the material path recovers them from `model_render`'s separate
    /// `is_additive` flag. Taking only the enum made this function *unable* to be right for an
    /// additive batch — every `Spells\` ground quad is mode 4, so Arcane Explosion / Blast Wave /
    /// Battle Shout drew their black-backed additive art alpha-blended: an opaque black tile.
    /// Keeping it in the signature is what stops the next caller repeating it.
    pub fn from_model(blend: ModelBlend, additive: bool) -> Self {
        if additive {
            // `BLEND_ADD` is byte-for-byte the material path's additive: the shader gamma-
            // premultiplies (`rgb·α`) and returns α = 0, turning `(One, 1−srcα)` into pure
            // addition — the same fold `specialize` gates on marker bit 2.
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

/// The fog COLOUR policy for one draw — `params.x`/`params.y` of the effect shader; each
/// variant is one canonical row of the render-world params uniform.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EffectFog {
    /// File flag 0x8 ("unfogged") — fog disabled outright (params.x = 0). Also the decal
    /// family's and foam's verified state.
    Off,
    /// The ordinary day-night scene fog (params.x = 1) — Alpha/Opaque blends.
    Scene,
    /// Fog toward BLACK (params.x = 2) — an Add-blend emitter fades under a veil instead of
    /// gaining grey (the same per-blend fog table M2 batches take, `0x70baf0`).
    Black,
    /// Fog toward WHITE (params.x = 3) — the `0x70baf0` table's Mod policy (a multiplier fades
    /// to the identity, not to the scene colour). Ground-fx decals with `ModelBlend::Mod`.
    White,
    /// Fog toward GREY-0.5 (params.x = 4) — the table's Mod2x policy (grey is 2·src·dst's
    /// neutral). Ground-fx decals with `ModelBlend::Mod2x`.
    Grey,
    /// Rain's FORCED grey fog (scene fog off; params.y = 1 with zw = 70..75) — under Mod2x the
    /// grey-0.5 fog colour is neutral, so this IS the streak/patter distance fade
    /// (`0x59d350`; the row values live with their law in `weather::precip`).
    Rain,
}

impl EffectFog {
    /// The policy for one particle/ribbon def — the exact table `particle_material` applied
    /// (file flag 0x8 wins, then Add ⇒ black, else scene).
    pub fn for_blend(flags: u32, blend: ParticleBlend) -> Self {
        if flags & 0x8 != 0 {
            EffectFog::Off
        } else if matches!(blend, ParticleBlend::Add) {
            EffectFog::Black
        } else {
            EffectFog::Scene
        }
    }

    /// The `0x70baf0` fog policy baked into a model material's `clutter_fade.z` bits 4..7
    /// (`wow_model.wgsl:761` decodes the same field): 0 scene, 1 black, 2 white, 3 grey,
    /// 4 unfogged — the ground-fx decal reads its part's authored policy through this.
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

/// Where one draw's light comes from — the lane's third per-draw identity axis, beside
/// [`EffectBlend`] and [`EffectFog`].
///
/// The reference has no particle material: it synthesizes an `M2Material` from the emitter's file
/// record every draw and runs the ordinary per-batch state producer over it, so `GL_LIGHTING`
/// lands on a particle quad exactly as it does on a mesh submesh — and the light it lands with is
/// the **model's own light node**, not the scene's (`0x672a20`). Which
/// node the model is on is what these three variants name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum EffectLighting {
    /// The authored colour burns as it is. The lane's default, and every family but one: ribbons,
    /// decals, the rings/reticle and precipitation are all authored to burn at their own colour,
    /// as is an M2 emitter that SETS the file's unlit bit 0x1 (the fire and spell corpus, which is
    /// why it stays bright at night).
    #[default]
    None,
    /// The **scene's** light, applied per view in the fragment shader against the world up axis —
    /// the outdoor lane, and the one a booth's own light buffer overrides. 400 of the corpus's
    /// 7792 emitters clear the unlit bit and land here, nearly all of them `World\` environment
    /// sheets: waterfall spray, chimney smoke, blown dust, snow, which read as full-white cutouts
    /// against shaded terrain when the term is missing (the Zul'Gurub waterfall foam).
    /// [`EffectDraw::lit`] is this variant and only this variant.
    Scene,
    /// The emitter's model carries its **own committed light**: it stands inside a WMO room, where
    /// the reference lights the object from its light node's footprint-MOCV words on a fixed axis
    /// plus the room's MOLT points, and never from the day/night sun (the WENTITY provider
    /// `0x6a7300`'s interior leg).
    ///
    /// A particle quad carries exactly one normal, so that whole node collapses to three floats
    /// ([`crate::interior::ParticleLight`]) and the producer has **already folded them into this
    /// draw's vertex colours** — which is why this shares a pipeline with [`Self::None`]: there is
    /// nothing left for the shader to do. Per-draw constants belong in the vertex stream on a lane
    /// whose whole design is one shared buffer of them.
    Committed,
}

/// A per-emitter light-buffer override: this emitter's fragment reads THIS `WowLight` blob
/// instead of the world's shared one. The glue-scene booths are the one author (decision 0539
/// §5 — their braziers are fogged by the SCENE's own light buffer, the ModelFFX fog that
/// covers the whole backdrop model); the call site inserts it on the spawned emitter entity.
#[derive(Component, Clone)]
pub struct EffectLightOverride(pub Buffer);

/// One draw of the shared lane: a contiguous vertex range, its texture/blend/fog identity, and
/// the sort point the render-world queue keys it by.
pub struct EffectDraw {
    /// The MAIN-world camera entity whose view this draw belongs to (the world camera, or a
    /// booth camera for a booth-layered emitter — the sim already resolves this per emitter).
    /// The render-world phase lookup keys on the retained view's main entity, so no
    /// `RenderLayers` plumbing is needed render-side.
    pub cam: Entity,
    pub(crate) texture: AssetId<Image>,
    pub(crate) blend: EffectBlend,
    pub(crate) topology: EffectTopology,
    pub(crate) fog: EffectFog,
    /// Does the SHADER multiply this draw's RGB by the scene's light? True for
    /// [`EffectLighting::Scene`] alone — a pipeline-key axis, so the merge walk separates lit from
    /// unlit runs through the pipeline id. A [`EffectLighting::Committed`] draw is `false`: its
    /// light is already in the vertices.
    pub lit: bool,
    /// The cloud's sort point — the emitter anchor / ribbon head node / decal center, exactly
    /// the sort point the material path used.
    pub(crate) anchor: Vec3,
    /// The ladder rung added to the view-space sort distance — owner-last for
    /// emitters, the decal constants (ring/ground-fx 8192, shadow 4096), foam's +1 water
    /// tie-break; `sky_order`'s sign law (positive draws later).
    pub(crate) bias: f32,
    /// The rasterizer `DepthBiasState` constant for this draw's pipeline: the
    /// coplanar decals keep the depth-offset half their materials carried (projected verts are
    /// exact sub-pieces of drawn ground — clip-interpolated vertices land within ULPs of it);
    /// everything free-floating passes 0. Nonzero ALSO selects the decal transform:
    /// the draw's verts skip the cam-relative rebase and run the world-mesh `clip_from_world`,
    /// so the depth tie the bias settles is against the same arithmetic.
    pub(crate) raster_bias: i32,
    /// The slope-scale half of the settle — see [`EffectDrawSpec::raster_slope`].
    pub(crate) raster_slope: f32,
    /// Are this draw's vertices **already camera-relative**? See [`EffectDrawSpec::cam_relative`].
    pub(crate) cam_relative: bool,
    /// Does this draw ignore the depth buffer? See [`EffectDrawSpec::no_depth_test`].
    pub(crate) no_depth_test: bool,
    /// Vertex range in [`EffectQuads::verts`] (a multiple of 4 for quads, 3 for tris).
    pub range: Range<u32>,
    /// The producing entity — the phase probe's identity for this item (`item.entity.1`, so a
    /// phase line still names the pool that produced it).
    pub main_entity: Entity,
    /// [`EffectLightOverride`]'s buffer, when the producer carries one (`None` = the world's
    /// shared light buffer).
    pub(crate) light: Option<Buffer>,
    /// The target-pixel rectangle this draw is clipped to — see [`EffectDrawSpec::clip`].
    pub(crate) clip: Option<Vec4>,
}

/// The frame's shared stream. Cleared at the top of `PostUpdate`'s effect set
/// ([`begin_effect_frame`]), filled by the family systems, copied to the render world in
/// `ExtractSchedule`.
#[derive(Resource)]
pub struct EffectQuads {
    pub verts: Vec<EffectVertex>,
    pub draws: Vec<EffectDraw>,
    /// Has [`begin_effect_frame`] run yet **this** frame? A writer that commits while this is
    /// `false` ran BEFORE the clear, so everything it pushed is about to be erased — a silent,
    /// total loss of that lane's frame, with its arithmetic entirely correct.
    ///
    /// This is the shape of B161: the chain beam declared its pose dependencies but not
    /// `.after(begin_effect_frame)`, the clear carried one extra `.after` the beam did not, and
    /// the beam became runnable first. Nothing warned; the lane simply never drew. The ordering
    /// is unenforceable by the type system (any writer can forget an edge), so the protocol is
    /// asserted here instead — [`Self::commit`] refuses a pre-clear write in debug builds, and
    /// [`clear_effect_frame_flag`] arms it again in `Last`.
    cleared_this_frame: bool,
}

impl Default for EffectQuads {
    fn default() -> Self {
        Self {
            verts: Vec::new(),
            draws: Vec::new(),
            // Armed, not tripped: a fixture that commits into a bare stream (most of the family's
            // unit tests) is testing geometry, not schedule order, and has no clear to run. The
            // `Last` re-arm is what makes the flag mean anything, and it only exists in an app
            // carrying `ParticlePlugin` — where the very first frame's clear precedes any writer
            // regardless, and a mis-ordered writer trips on frame two and every frame after.
            cleared_this_frame: true,
        }
    }
}

/// Everything about one draw except its vertex range — the argument bundle `commit_quads` /
/// `commit_tris` close a range with.
pub struct EffectDrawSpec {
    pub cam: Entity,
    pub texture: AssetId<Image>,
    pub blend: EffectBlend,
    pub fog: EffectFog,
    /// Which light this draw's authored colour meets — see [`EffectLighting`]. The scene's term is
    /// `clamp(ambient + diffuse·max(N·L,0))` against the **world up axis**, the same shape
    /// `wow_model.wgsl` applies to a mesh: the reference's quad writer uploads one constant normal
    /// for the whole draw — world +Z carried into eye space, against a light carried into the same
    /// frame, so the product is the view-invariant `worldUp · worldLightDir` (`0x7b3fd0`; decision
    /// 1696 supersedes 0975's camera-facing reading). The
    /// gate on whether an emitter takes any of it is the file's unlit bit
    /// ([`benilla_formats::ParticleEmitterDef::lit`], byte law there).
    pub lighting: EffectLighting,
    pub anchor: Vec3,
    pub bias: f32,
    pub raster_bias: i32,
    /// The rasterizer `DepthBiasState` **slope-scale** half of this draw's settle — the term that
    /// scales with the primitive's own depth slope, and the one a near-horizontal decal seen at a
    /// grazing angle actually needs (a constant cannot follow a surface whose depth gradient runs
    /// away from the eye). `0.0` for every family with no coplanar receiver.
    ///
    /// It exists because the reference has one: the sole `glPolygonOffset` site (`0x59bf0a`, a
    /// hardcoded `0xc0800000`) arms `factor = −4.0` alongside the units term whenever EGxRs id `0`
    /// is set non-zero, which is what the foam draw's `0x68fd0f` does. 1809.
    pub raster_slope: f32,
    /// Set when the producer has **already** written camera-relative vertices, so
    /// [`super::render`]'s rebase must skip this draw. The default (`false`) is the lane's
    /// contract: producers write ABSOLUTE world positions and the rebase subtracts the camera
    /// on the upload copy.
    ///
    /// It exists for **precision**, and precisely one family needs it. `EffectVertex::pos` is
    /// f32, WoW world coordinates run to ±17066 yd, and the rebase happens *after* the producer's
    /// rounding — so a vertex offset smaller than an ULP at the writing position is simply lost.
    /// Every other family's geometry is centimetre-scale or bigger and never notices. The snow
    /// flake is the lane's first **sub-centimetre** geometry: reproducing the reference's point
    /// sprite (14 px at the eye) puts a near flake's half-extent at ~1.6 mm, which at Kharanos's
    /// ~5600-yd coordinates is **3 ULPs** and at a map corner **0.8** — measured: a 14 px sprite
    /// 0.3 yd from the eye loses 2.4 px of width there and 6+ px at the map edge, i.e. visible
    /// size flicker and dropouts on exactly the flakes the eye is drawn to. Writing the offsets
    /// camera-relative keeps every term small and the arithmetic exact.
    ///
    /// The sort [`Self::anchor`] stays absolute either way — it is not a vertex.
    pub cam_relative: bool,
    /// **Draw over everything** — force the depth COMPARE to `Always`, so this draw is never
    /// occluded by anything already in the depth buffer. Nothing is depth-*written* either way on
    /// a blended draw, so it occludes nothing in turn.
    ///
    /// The lane's default is `false`, and it stays the default: a depth-tested transparent is what
    /// every particle, ribbon, decal and streak wants. One family sets it — the weapon swing trail,
    /// whose callback writes EGxRs id `0x10` to `0` (`0x6c686e`; GL `glDisable(GL_DEPTH_TEST)`,
    /// D3D `ZFUNC = D3DCMP_ALWAYS`) so the arc is never eaten by the swinging character's own
    /// shoulder.
    pub no_depth_test: bool,
    pub main_entity: Entity,
    pub light: Option<Buffer>,
    /// **Clip this draw to a rectangle of its RENDER TARGET, in target pixels** — `(min.x,
    /// min.y, max.x, max.y)`, `None` for the whole target, which is every world family.
    ///
    /// It exists for the UI model tiles: every visible `<Model>` pane renders
    /// into its own cell of ONE shared atlas, and a pane's particles are quads in that atlas's
    /// space — so a cloud that reaches past its cell lands in the cell **next to** it, which the
    /// composite hands to a different widget. The reference cannot have this: it draws each
    /// `<Model>` straight into the back buffer with the widget's own rect as the VIEWPORT
    /// (`0x59c730`), and the scissor eats the overflow. One shared
    /// camera cannot carry a viewport per pane, so the clip rides the draw instead and the
    /// fragment discards outside it — the same picture, at the same rank.
    ///
    /// B379: the pet bar's autocast shine (`UI-AutoCastButton.m2` — four additive spline
    /// emitters, no render batch at all) circles its button on the very edge with an ~8 px
    /// half-extent, which spilled six pixels of golden `GlowStar` across the 2-texel gutter and
    /// down the LEFT edge of whatever cooldown pane the shelf packed beside it.
    pub clip: Option<Vec4>,
}

impl EffectQuads {
    /// Open a draw: remember where its vertices start.
    pub fn begin(&self) -> u32 {
        self.verts.len() as u32
    }

    /// Close a quad draw over everything pushed since `begin`. A range that gained no vertices
    /// commits nothing — the idle steady state costs zero here (the a5521180 law, structural).
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
             (see `EffectQuads::cleared_this_frame` — this is B161's failure mode)."
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

/// Clear the stream for a new frame — scheduled before every family's writer, so the writer
/// order between them stays free.
pub fn begin_effect_frame(mut quads: ResMut<EffectQuads>) {
    quads.verts.clear();
    quads.draws.clear();
    quads.cleared_this_frame = true;
}

/// Disarm the write-order tripwire for the next frame — `Last`, after the render world has
/// extracted this frame's stream. See [`EffectQuads::cleared_this_frame`].
pub fn clear_effect_frame_flag(mut quads: ResMut<EffectQuads>) {
    quads.cleared_this_frame = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The additive flag OVERRIDES the folded enum. `ModelBlend::Blend` means
    /// "alpha-blended **or** additive" — M2 modes 2/3/4 all land there — so a mapping that reads
    /// only the enum cannot be right. Every `Spells\` flat ground quad in the 1.12.1 corpus is
    /// mode 4 (`m2batch`: ArcaneExplosion_Base, BattleShout_Cast_Base, …), and drawing those
    /// alpha-blended painted their black-backed additive art as an opaque black tile.
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

    /// The non-additive law is unchanged — `model_render.rs`'s mapping, with `AlphaTest` folded
    /// to `Alpha` (the lane has no Mask variant; flat `Spells\` quads are blend batches).
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

    /// **B161, as an executable record.** The chain beam shipped with correct arithmetic and no
    /// `.after(begin_effect_frame)` edge, and drew nothing at all — for weeks, silently.
    ///
    /// The mechanism is not "the writers race": it is that the clear carries one dependency the
    /// writer does not (`face_billboards`), so the writer becomes *runnable first* and the clear
    /// lands on top of it. This pair pins both halves — the failure mode, and the edge that fixes
    /// it. The graph below is the real registration shape of `ParticlePlugin` (the clear) and
    /// `EntitiesPlugin` (the beam sim), transcribed.
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

        /// `edged`: whether the writer declares the load-bearing `.after(begin_effect_frame)`.
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
                    // The extra dependency the beam sim lacked — the whole mechanism.
                    .after(face_billboards),
            );
            app.add_systems(Last, clear_effect_frame_flag);
            // Two frames: the first arms the tripwire (a bare stream defaults to armed, and the
            // `Last` re-arm has not run yet), the second is any ordinary frame.
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

        /// Without it, the write precedes the clear — the tripwire that now names the bug.
        #[test]
        #[should_panic(expected = "effect-stream write before `begin_effect_frame`")]
        fn without_the_edge_the_tripwire_names_it() {
            let _ = run(false);
        }
    }
}

/// **Draw a batch of quads or triangles in the effect lane** — the stream's face for everything
/// that is not a particle.
///
/// Six lanes push into this buffer and every one of them wrote the same eleven-field
/// [`EffectDrawSpec`] literal, of which three fields are the same value at every gameplay site
/// (`lighting: None`, `cam_relative: false`, `light: None` — the exceptions are all engine-internal:
/// M2 emitters light, the snow slab writes camera-relative, the booths bind their own buffer).
/// A caller was choosing eight things and restating three.
///
/// The builder is what keeps [`EffectBlend`] and [`EffectFog`] off the doorway: a lane says
/// `.additive()` rather than naming an enum, and the defaults it does not mention are the lane's
/// contract rather than a line it had to copy correctly.
#[derive(bevy::ecs::system::SystemParam)]
pub struct WorldEffectDraw<'w> {
    quads: ResMut<'w, EffectQuads>,
}

impl<'w> WorldEffectDraw<'w> {
    /// Open a batch drawn through `cam` with `texture`. Nothing is committed until [`EffectBatch`]
    /// is closed with `tris()` or `quads()`, and a batch that gained no vertices commits nothing.
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

/// One open batch. Field-free by construction: a caller holds it just long enough to push
/// vertices and close it, so the type never has to be named.
pub struct EffectBatch<'a> {
    quads: &'a mut EffectQuads,
    start: u32,
    spec: EffectDrawSpec,
}

impl EffectBatch<'_> {
    /// Add this batch's colour to the framebuffer — glows, rings, beams.
    pub fn additive(mut self) -> Self {
        self.spec.blend = EffectBlend::Add;
        self
    }

    /// Blend it over what is already drawn, `SRC_ALPHA / INV_SRC_ALPHA` — the lane's default,
    /// spelled out for a producer whose blend is a *verified* state rather than an unremarked one
    /// (the swing trail's EGxRs `0x07 = 2`, which reads like it ought to be additive and is not).
    pub fn alpha(mut self) -> Self {
        self.spec.blend = EffectBlend::Alpha;
        self
    }

    /// Multiply it into what is already drawn — the blob shadow's darkening.
    pub fn multiply(mut self) -> Self {
        self.spec.blend = EffectBlend::Multiply;
        self
    }

    /// An explicit blend, for a lane whose authored data carries one.
    pub fn blend(mut self, blend: EffectBlend) -> Self {
        self.spec.blend = blend;
        self
    }

    /// An explicit fog policy; the default is [`EffectFog::Off`], which is what every decal and
    /// overlay wants (they burn at their own colour).
    pub fn fog(mut self, fog: EffectFog) -> Self {
        self.spec.fog = fog;
        self
    }

    /// Where this batch sorts from — a world point, not a vertex. Transparents are ordered by the
    /// distance to this.
    pub fn anchored(mut self, at: Vec3) -> Self {
        self.spec.anchor = at;
        self
    }

    /// This batch's rung on the draw-order ladder: the transparent sort bias and the rasterizer
    /// depth-bias constant (`crate::sky_order::Rung`). The slope-scale half stays 0 — only the
    /// foam lane carries one, and it builds its spec directly.
    pub fn rung(mut self, sort: f32, raster: i32) -> Self {
        self.spec.bias = sort;
        self.spec.raster_bias = raster;
        self
    }

    /// Draw over everything, occluded by nothing — see [`EffectDrawSpec::no_depth_test`].
    pub fn over_everything(mut self) -> Self {
        self.spec.no_depth_test = true;
        self
    }

    /// The entity this draw belongs to, for the render world's per-item bookkeeping.
    pub fn owner(mut self, entity: Entity) -> Self {
        self.spec.main_entity = entity;
        self
    }

    /// Push vertices already built elsewhere — the decal lanes project once and cache.
    pub fn vertices(&mut self, verts: &[EffectVertex]) {
        self.quads.verts.extend_from_slice(verts);
    }

    /// Push vertices from an iterator.
    pub fn extend(&mut self, verts: impl IntoIterator<Item = EffectVertex>) {
        self.quads.verts.extend(verts);
    }

    /// The vertex sink, for a producer that writes in place rather than yielding — the chain
    /// beam builds each strand into it directly.
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
