//! The per-display model cache: a creature, GameObject or held-item display id resolves to a
//! [`DisplayModel`] whose spawn parts are built once, when its M2 or WMO loads, and shared by every
//! entity of that display; [`super::attach`] gives each entity its visual from them.

use avian3d::prelude::Collider;
use benilla_assets::coords::wow_to_bevy;
use benilla_assets::{
    M2Model, ModelAnimations, ModelEmitter, ModelSkeleton, ModelSubmesh, WmoModel,
};
use benilla_formats::{
    CharSkinSlot, CollisionMesh, CreatureCatalog, GameObjectCatalog, ModelBlend, NpcAppearance,
};
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;

use benilla_assets::materials::WowModelMaterial;
use benilla_assets::{m2_url, skin_url, wmo_url};

/// A display's model asset: an M2 (creatures, most GameObjects), a WMO (building GameObjects), or
/// none when the display names no model.
pub(crate) enum ModelHandle {
    M2(Handle<M2Model>),
    Wmo(Handle<WmoModel>),
    None,
}

/// One spawn part of a display model: a submesh's render forms and built materials.
#[derive(Clone)]
pub(super) struct EntityPart {
    pub(super) mesh: Handle<Mesh>,
    /// The resident CPU geometry, cloned onto each part as [`benilla_world::interact::PickMesh`]:
    /// the render forms are `RENDER_WORLD`-only, leaving pickers no mesh to ray-cast.
    pub(super) geometry: std::sync::Arc<benilla_formats::RenderSubmesh>,
    /// The static form's build-time bound, the mesh itself being `RENDER_WORLD`-only.
    pub(super) aabb: Option<bevy::camera::primitives::Aabb>,
    /// The skinned form of [`Self::mesh`], which an animated M2 instance draws.
    pub(super) skinned_mesh: Option<Handle<Mesh>>,
    pub(super) material: Handle<WowModelMaterial>,
    /// The M2 variant the interior classifier ([`benilla_world::interior`]) swaps to inside a WMO
    /// room: the plain day/night matte, sun ×1.0. A WMO part's interior is baked into `material`.
    pub(super) material_interior: Option<Handle<WowModelMaterial>>,
    /// The interior-prop variant, whose `MeshTag` the shader reads as an SH-probe slot. Units and
    /// GameObjects alike take it: the reference registers every entity M2 with one entity-node fill
    /// (`Node::SetModel` `0x6716f0`), so one law lights them all indoors (`0x6a7300`).
    pub(super) material_interior_bake: Option<Handle<WowModelMaterial>>,
    /// The bake variant's blend form, so a fading bake-lit part keeps its probe light.
    pub(super) material_interior_bake_blend: Option<Handle<WowModelMaterial>>,
    /// The exterior material's blend form, worn during the spawn appear-fade (`RenderFade`).
    pub(super) fade_blend: Option<Handle<WowModelMaterial>>,
    /// The depth-prime form ([`benilla_world::model_render::zfill_material`], the reference's
    /// `M2UseZFill` clone, `0x707f7d`–`0x708072`): a translucent part's colour-masked, z-writing
    /// child draws it first, so a fading or stealthed body blends as one layer. `None` when the
    /// batch disables z-write or z-test (the reference's `(flags & 0x10) == 0` gate) or cannot fade
    /// (Mod/Mod2x).
    pub(super) zfill: Option<Handle<WowModelMaterial>>,
    pub(super) blend: ModelBlend,
    /// Additive blending (M2 blend mode 3 `NoAlphaAdd` or 4 `Add`), which [`ModelBlend`]'s single
    /// `Blend` variant folds in with alpha: anything re-deriving a blend state from `blend` needs
    /// this too, or an additive batch draws alpha-blended.
    pub(super) additive: bool,
    /// Two-sided (M2 material flag `0x04`), kept so a per-appearance material swap preserves it.
    pub(super) two_sided: bool,
    /// The part's `skinSectionId` (geoset id). A player body's attach spawns only the selected
    /// geosets; every other model draws whole, even with several authored (101 creature models):
    /// the reference's visibility array starts all visible and only the character compositor
    /// writes it, which a creature never reaches (`0x5fb200`).
    pub(super) geoset_id: u16,
    /// The character texture slot (type 1 body, 6 hair) a player's attach re-skins per appearance.
    pub(super) char_slot: Option<CharSkinSlot>,
    /// A billboard batch's pivot and facing. Never spawn it as an ordinary child: its mesh is
    /// centred at the bone pivot and a following `BillboardCard` owns its transform.
    pub(super) billboard: Option<benilla_assets::BillboardInfo>,
    /// Geometry welded to a billboard bone the card split refused, which draws right only through a
    /// joint palette ([`DisplayModel::welds_billboard`]).
    pub(super) welded_billboard: bool,
    /// The animated material alpha per sequence, colour alpha × transparency weight.
    pub(super) alpha_anim: Option<std::sync::Arc<benilla_formats::AlphaAnim>>,
    /// The animated RGB tint (the M2Color track, time-varying only), its first key seeded into the
    /// materials.
    pub(super) rgb_anim: Option<std::sync::Arc<benilla_formats::RgbAnim>>,
    /// [`Self::rgb_anim`] per file sequence slot, only where the slots disagree: four of the five
    /// such batches are empty in slot 0, so a shared material would seed white (Freezing Trap).
    pub(super) rgb_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 3]>>>,
    /// The texture-transform translation that scrolls the batch's UVs (`0x70b740`).
    pub(super) uv_anim: Option<std::sync::Arc<benilla_formats::UvAnim>>,
    /// [`Self::uv_anim`] per file sequence slot, only where the slots disagree, so an effect
    /// advancing `Stand` → `Hold` → `Decay` reads the slot it plays.
    pub(super) uv_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    /// The texture transform's rotation loop per file sequence slot, and below it the scaling one;
    /// three effect models author them (`benilla-extract fxuvscan`), Shield Wall's halo among them.
    pub(super) uv_rot_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 4]>>>,
    pub(super) uv_scale_seq: Option<std::sync::Arc<benilla_formats::SeqLoops<[f32; 2]>>>,
    /// A flat ground-plane quad ([`benilla_formats::GroundQuad`]), which a base-anchored spell
    /// effect draws as a terrain-draping decal (`benilla_world::ground_fx`).
    pub(super) ground_quad: Option<benilla_formats::GroundQuad>,
}

impl EntityPart {
    /// The four texture-transform channels as the engine's lanes take them, the one place a spawner
    /// reads them. Ask `UvLoops::animates`: a scale-only transform or a dead slot-0 translation is
    /// `None` on the channel you would check first.
    pub(super) fn uv_loops(&self) -> benilla_world::doodad_anim::UvLoops {
        benilla_world::doodad_anim::UvLoops {
            seqs: self.uv_seq.clone(),
            single: self.uv_anim.clone(),
            rot: self.uv_rot_seq.clone(),
            scale: self.uv_scale_seq.clone(),
        }
    }
}

/// Everything needed to render one display id, built once and shared by its entities; `parts` is
/// `None` while the asset loads, and empty for a model with no geometry.
pub(crate) struct DisplayModel {
    pub(crate) handle: ModelHandle,
    /// The model's directory, where its skin textures live.
    pub(super) dir: String,
    /// The creature's `Monster1/2/3` skin variation names.
    pub(super) skins: [Option<String>; 3],
    /// A humanoid NPC's body from `CreatureDisplayInfoExtra` (race, sex, customization, the baked
    /// body atlas), skinned and geoset-filtered as a player's is; a player's own rides the wire.
    pub(super) npc_appearance: Option<NpcAppearance>,
    /// A held item's object skin, the `ItemDisplayInfo` texture bound to type-2 batches
    /// ([`CharSkinSlot::Object`]) from [`Self::dir`].
    pub(super) object_texture: Option<String>,
    pub(super) parts: Option<Vec<EntityPart>>,
    pub(super) emitters: Vec<ModelEmitter>,
    pub(super) ribbons: Vec<benilla_assets::ModelRibbon>,
    /// The model's M2 lights, the dynamic light an entity carries: a held torch
    /// (`Club_1H_Torch_A_01.m2`, one `type == 1` point light) lights the ground around its bearer.
    pub(super) lights: Vec<benilla_assets::ModelLight>,
    /// The model-local collider from the collision hull, GameObjects only; a hull-less model does
    /// not collide, so a herb is walk-through while a chest, vein or door is solid.
    pub(super) collider: Option<Collider>,
    pub(super) skeleton: ModelSkeleton,
    /// Attachment points (id → bone and Bevy-space offset) for each instance's `BoneAttach`.
    pub(super) attachments: Vec<benilla_assets::ModelAttachment>,
    /// Animation-event markers (FourCC → bone and offset, in file order, first match wins), such as
    /// the missile launch points `$CSL`/`$CSR`/`$CST`/`$BWR`.
    pub(super) markers: Vec<benilla_assets::ModelMarker>,
    pub(super) inverse_bindposes: Option<Handle<SkinnedMeshInverseBindposes>>,
    pub(super) animations: Option<ModelAnimations>,
    /// The first sequence's authored duration, the spell-effect clock for a model whose sequences
    /// build no clip (the eat/drink tankard: 6.667 s, no bone keys).
    pub(crate) first_seq_span: Option<f32>,
    /// The camera framing pivot's height in model-local yards before scale (`0x50cbc0`), stamped on
    /// each instance as `CameraPivot`; `0.0` without bounds.
    pub(super) pivot_height_local: f32,
    /// How far that pivot drops while swimming: the reference's swim preset `cam+0x124` is the
    /// standing height less `StandSeq.max.z − SwimSeq.max.z` (`0x50ccf6`), picked on
    /// `MOVEFLAG_SWIMMING` (`0x50f880`); `0.0` without a Swim sequence, the reference's own guard.
    pub(super) swim_pivot_drop_local: f32,
    /// The selection ring's radius in model-local yards before scale, the Stand footprint
    /// `sqrt(0.5 · sqrt(dx² + dy²))`, scaled by `OBJECT_FIELD_SCALE_X` (`0x608e00`/`0x60aee0`).
    pub(super) ground_radius_local: f32,
    /// The authored portrait camera the portrait booth frames through (`0x713540`).
    pub(super) portrait_camera: Option<benilla_assets::PortraitCamera>,
    /// Camera-table index 1, the rig a `<PlayerModel>` widget renders through (`0x505890`); without
    /// it the reference uses its fixed fallback camera.
    pub(super) pane_camera: Option<benilla_assets::PortraitCamera>,
    /// A bow's `$WTT`/`$WTB` string anchors (`0x611ff0`), `[top, bottom]` as
    /// `(bone, model-local Bevy position)`.
    pub(super) string_anchors: Option<[(u16, Vec3); 2]>,
    /// A fishing pole's `$CCH` line anchor (`0x61f780`) in mesh-frame Bevy space.
    pub(super) cch_marker: Option<Vec3>,
    /// The authored bbox z-extent in model-local yards before scale: a model with no PlayerName
    /// attachment anchors overhead text at `feet + scale × this × 1.25` (`0x608640`).
    pub(super) bbox_z_local: f32,
    /// The Stand sequence box's z-extent, the chat bubble's anchor height (`0x711a20`): a file
    /// constant, so the bubble never moves with the pose as the overhead name does.
    pub(super) stand_box_z_local: f32,
    /// The vertex-box centre in Bevy model space, the interior fold's MOLR reference point for a
    /// GameObject (the reference's `[def+0x5c]` anchor).
    pub(super) bake_center_local: Vec3,
    /// MD20 `GlobalModelFlags & 3` (`0x7106c0`): `1` pitches to the slope, `3` pitches and rolls,
    /// anything else stays level.
    pub(super) terrain_tilt: u8,
    /// A `Character\` model, the gate for the customization pipeline. It follows the display, not
    /// the entity kind (a druid in bear form wears a creature model), as the reference's race and
    /// gender getters read the display's row (`0x60c690`).
    pub(super) is_character_body: bool,
}

impl DisplayModel {
    /// Whether a built part is welded to a billboard bone: it has no rigid placement (the reference
    /// blends it per vertex), so even the item lane must rig for it (seven shoulder models).
    pub(super) fn welds_billboard(&self) -> bool {
        self.parts
            .as_ref()
            .is_some_and(|p| p.iter().any(|part| part.welded_billboard))
    }

    /// Whether the display names a model file. An empty `parts` from a named model is that model's
    /// answer, draw nothing; [`empty_display`] (no catalog row, a zero-scale row, no model path) is
    /// an unresolved display, and earns the debug cube.
    pub(super) fn names_a_model(&self) -> bool {
        !matches!(self.handle, ModelHandle::None)
    }
}

/// A creature display's M2, its skins filled at build; a missing or zero-scale row is empty.
pub(super) fn new_creature_display(
    catalog: &CreatureCatalog,
    display_id: u32,
    asset_server: &AssetServer,
) -> DisplayModel {
    match catalog.model(display_id) {
        Some(m) if m.scale > 0.0 => DisplayModel {
            handle: ModelHandle::M2(asset_server.load(m2_url(&m.model_path))),
            dir: model_dir(&m.model_path).to_string(),
            skins: m.textures,
            npc_appearance: m.npc_appearance,
            is_character_body: m
                .model_path
                .get(..10)
                .is_some_and(|p| p.eq_ignore_ascii_case("character\\")),
            ..empty_shell()
        },
        _ => empty_display(),
    }
}

/// A GameObject display's M2 or WMO; one with no model path is empty.
pub(super) fn new_gameobject_display(
    catalog: &GameObjectCatalog,
    display_id: u32,
    asset_server: &AssetServer,
) -> DisplayModel {
    let Some(path) = catalog.model_path(display_id) else {
        return empty_display();
    };
    let handle = if path.to_ascii_lowercase().ends_with(".wmo") {
        ModelHandle::Wmo(asset_server.load(wmo_url(path)))
    } else {
        ModelHandle::M2(asset_server.load(m2_url(path)))
    };
    DisplayModel {
        handle,
        ..empty_shell()
    }
}

/// A blank [`DisplayModel`] awaiting [`build_parts`], for constructors to override.
pub(crate) fn empty_shell() -> DisplayModel {
    DisplayModel {
        handle: ModelHandle::None,
        dir: String::new(),
        skins: Default::default(),
        npc_appearance: None,
        object_texture: None,
        parts: None,
        emitters: Vec::new(),
        ribbons: Vec::new(),
        lights: Vec::new(),
        collider: None,
        skeleton: ModelSkeleton::default(),
        attachments: Vec::new(),
        markers: Vec::new(),
        string_anchors: None,
        cch_marker: None,
        inverse_bindposes: None,
        animations: None,
        first_seq_span: None,
        pivot_height_local: 0.0,
        swim_pivot_drop_local: 0.0,
        ground_radius_local: 0.0,
        portrait_camera: None,
        pane_camera: None,
        bbox_z_local: 0.0,
        stand_box_z_local: 0.0,
        bake_center_local: Vec3::ZERO,
        terrain_tilt: 0,
        is_character_body: false,
    }
}

/// A display with no model, its `parts` already empty.
pub(super) fn empty_display() -> DisplayModel {
    DisplayModel {
        parts: Some(Vec::new()),
        ..empty_shell()
    }
}

/// Build a display's spawn parts once its asset and render forms are ready, filling a creature skin
/// slot from the display's variation (`<dir>\<name>.blp`); until then `parts` stays `None`.
pub(super) fn build_parts(
    dm: &mut DisplayModel,
    m2s: &Assets<M2Model>,
    wmos: &Assets<WmoModel>,
    // Entity displays request forms at priority 0, ahead of scenery placements (16 and up).
    forms: &mut benilla_world::model_forms::ModelForms,
    asset_server: &AssetServer,
    mats: &mut benilla_world::model_render::M2BatchMaterials,
    // Where the display's animated texture transforms go as its materials are built.
    uv: &mut benilla_world::model_render::EntityUvLane<'_>,
    // Only a GameObject builds the hull collider.
    gameobject: bool,
) {
    if !mats.ready() {
        return; // no shared light buffer yet; retry next frame
    }
    let mut emitters = Vec::new();
    let mut ribbons = Vec::new();
    let mut lights = Vec::new();
    let mut collider = None;
    let mut terrain_tilt = 0u8;
    let mut skeleton = ModelSkeleton::default();
    let mut attachments = Vec::new();
    let mut markers = Vec::new();
    let mut string_anchors = None;
    let mut cch_marker = None;
    let mut inverse_bindposes = None;
    let mut animations = None;
    let mut first_seq_span = None;
    let mut pivot_height_local = 0.0;
    let mut swim_pivot_drop_local = 0.0;
    let mut ground_radius_local = 0.0;
    let mut portrait_camera = None;
    let mut pane_camera = None;
    let mut bbox_z_local = 0.0;
    let mut stand_box_z_local = 0.0;
    let mut bake_center_local = Vec3::ZERO;
    let parts = match &dm.handle {
        ModelHandle::M2(h) => {
            let Some(model) = m2s.get(h) else {
                return; // still loading
            };
            // Every M2 lane can rig, so both forms are requested; static instances and billboard
            // cards draw the static one.
            if !forms.require_rigged(h, 0) {
                return; // forms still building
            }
            emitters = model.emitters.clone();
            ribbons = model.ribbons.clone();
            lights = model.lights.clone();
            terrain_tilt = (model.global_flags & 3) as u8;
            // Not the render sphere (`0xCC`), which sizes the corpse decal (`0x5d6fe0`).
            ground_radius_local = model.bounds.map_or(0.0, |b| b.ring_footprint);
            // Attachment 17's Z + 0.0972, about neck height on every character model, else 0.9 ×
            // the vertex-box Z extent, a box that spans every animation (`0x50cbc0`).
            pivot_height_local = model.bounds.map_or(0.0, |b| {
                b.pivot_z
                    .map(|z| z + 0.0972)
                    .unwrap_or_else(|| 0.9 * (b.bbox_max[2] - b.bbox_min[2]).max(0.0))
            });
            swim_pivot_drop_local = model.bounds.map_or(0.0, |b| b.swim_pivot_drop);
            bbox_z_local = model
                .bounds
                .map_or(0.0, |b| (b.bbox_max[2] - b.bbox_min[2]).max(0.0));
            // The same Stand box the ring's footprint reads.
            stand_box_z_local = model.bounds.map_or(0.0, |b| b.stand_box_z);
            bake_center_local = model.bounds.map_or(Vec3::ZERO, |b| {
                benilla_assets::coords::wow_to_bevy([
                    (b.bbox_min[0] + b.bbox_max[0]) * 0.5,
                    (b.bbox_min[1] + b.bbox_max[1]) * 0.5,
                    (b.bbox_min[2] + b.bbox_max[2]) * 0.5,
                ])
            });
            skeleton = model.skeleton.clone();
            attachments = model.attachments.clone();
            markers = model.markers.clone();
            string_anchors = model.string_anchors;
            cch_marker = model.cch_marker;
            inverse_bindposes = Some(model.inverse_bindposes.clone());
            animations = model.animations.clone();
            first_seq_span = model.first_seq_span;
            portrait_camera = model.portrait_camera;
            pane_camera = model.pane_camera;
            if gameobject {
                collider = model.collision.as_ref().and_then(model_local_collider);
            }
            let built = forms.slices(h);
            let (stat_forms, skin_forms) = (built.stat, built.skin.unwrap_or(&[]));
            model
                .submeshes
                .iter()
                .enumerate()
                .map(|(pi, sub)| {
                    let texture = resolve_skin(
                        sub,
                        &dm.dir,
                        &dm.skins,
                        dm.object_texture.as_deref(),
                        asset_server,
                    );
                    // The authored batch order (index + 1) biases the transparent sort, so coplanar
                    // transparent batches draw in file order instead of flipping a tie each frame.
                    let order = u16::try_from(pi + 1).unwrap_or(u16::MAX);
                    let v = mats.entity_variants(sub, texture, order, uv);
                    let v = v.expect("light buffer checked at entry");
                    EntityPart {
                        // Index-parallel with the submeshes; a miss draws nothing, never panics.
                        mesh: stat_forms
                            .get(pi)
                            .map(|(h, _)| h.clone())
                            .unwrap_or_default(),
                        geometry: sub.geometry.clone(),
                        aabb: stat_forms.get(pi).and_then(|(_, a)| *a),
                        skinned_mesh: skin_forms.get(pi).cloned(),
                        material: v.steady,
                        material_interior: Some(v.interior),
                        material_interior_bake: Some(v.interior_bake),
                        material_interior_bake_blend: Some(v.interior_bake_blend),
                        fade_blend: Some(v.fade_blend),
                        zfill: v.zfill,
                        blend: sub.blend,
                        additive: sub.additive,
                        two_sided: sub.two_sided,
                        geoset_id: sub.geoset_id,
                        char_slot: sub.char_slot,
                        billboard: sub.billboard.clone(),
                        welded_billboard: sub.geometry.welded_billboard,
                        alpha_anim: sub.alpha_anim.clone(),
                        rgb_anim: sub.rgb_anim.clone(),
                        rgb_seq: sub.rgb_seq.clone(),
                        uv_anim: sub.uv_anim.clone(),
                        uv_seq: sub.uv_seq.clone(),
                        uv_rot_seq: sub.uv_rot_seq.clone(),
                        uv_scale_seq: sub.uv_scale_seq.clone(),
                        ground_quad: sub.ground_quad,
                    }
                })
                .collect()
        }
        ModelHandle::Wmo(h) => {
            let Some(model) = wmos.get(h) else {
                return;
            };
            // WMO-display GameObjects never skin: static forms only.
            if !forms.require_static(h, 0) {
                return;
            }
            if gameobject {
                collider = model.collision.as_ref().and_then(model_local_collider);
            }
            let stat_forms = forms.slices(h).stat;
            model
                .submeshes
                .iter()
                .enumerate()
                .map(|(pi, sub)| EntityPart {
                    mesh: stat_forms
                        .get(pi)
                        .map(|(h, _)| h.clone())
                        .unwrap_or_default(),
                    geometry: sub.geometry.clone(),
                    aabb: stat_forms.get(pi).and_then(|(_, a)| *a),
                    geoset_id: sub.geoset_id, // 0 for WMO: no geoset selection
                    char_slot: None,          // WMO is never a character body
                    skinned_mesh: None,       // no skeleton to skin
                    // One material per batch, interior light baked per submesh (MOBA class, SIDN,
                    // WINDOW); batch order 0: one building, not the streamer's coplanar stacks.
                    material: mats
                        .steady(sub, sub.texture.clone(), 0)
                        .expect("light buffer checked at entry"),
                    material_interior: None, // interior is baked into `material`
                    material_interior_bake: None, // never the M2 footprint lane
                    material_interior_bake_blend: None,
                    fade_blend: None, // WMO-display GameObjects do not appear-fade
                    zfill: None,      // so no depth-prime form either
                    blend: sub.blend,
                    additive: false, // WMO MOMT has no additive mode
                    two_sided: sub.two_sided,
                    billboard: None,         // WMO groups have no billboard bones
                    welded_billboard: false, // so nothing is welded to one
                    alpha_anim: None,        // nor colour or weight loops
                    rgb_anim: None,
                    rgb_seq: None,
                    uv_anim: None, // nor texture transforms
                    uv_seq: None,
                    uv_rot_seq: None,
                    uv_scale_seq: None,
                    ground_quad: None, // the fx decal lane is M2-only
                })
                .collect()
        }
        ModelHandle::None => Vec::new(),
    };
    dm.parts = Some(parts);
    dm.emitters = emitters;
    dm.ribbons = ribbons;
    dm.lights = lights;
    dm.collider = collider;
    dm.skeleton = skeleton;
    dm.attachments = attachments;
    dm.markers = markers;
    dm.string_anchors = string_anchors;
    dm.cch_marker = cch_marker;
    dm.inverse_bindposes = inverse_bindposes;
    dm.animations = animations;
    dm.first_seq_span = first_seq_span;
    dm.pivot_height_local = pivot_height_local;
    dm.swim_pivot_drop_local = swim_pivot_drop_local;
    dm.ground_radius_local = ground_radius_local;
    dm.portrait_camera = portrait_camera;
    dm.pane_camera = pane_camera;
    dm.bbox_z_local = bbox_z_local;
    dm.stand_box_z_local = stand_box_z_local;
    dm.bake_center_local = bake_center_local;
    dm.terrain_tilt = terrain_tilt;
}

/// A model-local trimesh collider from the raw collision hull, through `wow_to_bevy` like the
/// rendered submeshes; `None` for a hull-less or degenerate hull, which does not collide.
fn model_local_collider(hull: &CollisionMesh) -> Option<Collider> {
    if hull.indices.len() < 3 {
        return None;
    }
    let verts: Vec<Vec3> = hull.positions.iter().map(|p| wow_to_bevy(*p)).collect();
    let tris: Vec<[u32; 3]> = hull
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    Some(Collider::trimesh(verts, tris))
}

/// A submesh's texture: its embedded one, else the display's creature variation
/// (`<dir>\<name>.blp`) for a skin slot, or its `ItemDisplayInfo` texture for an object slot (M2
/// type 2); `None` draws the material's muted fallback.
fn resolve_skin(
    sub: &ModelSubmesh,
    dir: &str,
    skins: &[Option<String>; 3],
    object_texture: Option<&str>,
    asset_server: &AssetServer,
) -> Option<Handle<Image>> {
    if sub.char_slot == Some(CharSkinSlot::Object) {
        return object_texture.map(|t| asset_server.load(skin_url(dir, t)));
    }
    match (&sub.texture, sub.skin_slot) {
        (Some(t), _) => Some(t.clone()),
        (None, Some(slot)) => skins
            .get(slot as usize)
            .and_then(|o| o.as_ref())
            .map(|name| asset_server.load(skin_url(dir, name))),
        (None, None) => None,
    }
}

/// `Creature\Kobold\Kobold.mdx` → `Creature\Kobold`, where the skin variations live.
fn model_dir(model_path: &str) -> &str {
    model_path
        .rsplit_once(['\\', '/'])
        .map(|(dir, _)| dir)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both kinds carry the same empty `parts` once built, so only the handle tells a display that
    /// named no model from one whose model draws nothing.
    #[test]
    fn only_a_display_with_no_model_file_earns_the_cube() {
        assert!(
            !empty_display().names_a_model(),
            "a display that resolved to no model must stay cube-eligible"
        );
        assert!(
            !DisplayModel {
                handle: ModelHandle::None,
                parts: Some(Vec::new()),
                ..empty_shell()
            }
            .names_a_model(),
            "the handle decides, not the parts"
        );
        assert!(
            DisplayModel {
                handle: ModelHandle::M2(Handle::default()),
                // Built with nothing to draw, like `InvisibleStalker.m2`'s zero batches.
                parts: Some(Vec::new()),
                ..empty_shell()
            }
            .names_a_model(),
            "a display that named an M2 is answered by that model, empty parts included"
        );
        assert!(
            DisplayModel {
                handle: ModelHandle::Wmo(Handle::default()),
                ..empty_shell()
            }
            .names_a_model(),
            "a WMO display names a model too"
        );
    }
}
