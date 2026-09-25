//! UI model tiles: the renderer for a `<Model>` widget's M2, from the cooldown sweep, the autocast
//! shine, the pings, the item-push card and the map arrow to an addon's `CreateFrame("Model")`.
//!
//! The reference draws a pane straight into the back buffer, every batch once and LEQUAL over a
//! depth buffer cleared for the widget's rect (`0x76d240`, `0x70b360`), through the file camera
//! the widget names or, with none, orthographically over the frame's rect. Here each pane renders
//! into its own cell of one shared atlas, through the sheet's one orthographic camera or a pooled
//! perspective one, and [`compose_tiles`] draws each cell as a quad at the pane's callback rank.
//! `PlayerModel`, `DressUpModel` and `TabardModel` are booths instead (`crate::portrait`): the
//! reference frames them through camera 1 frozen at load.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{OrthographicProjection, Projection, RenderTarget, ScalingMode};
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use bevy::render::renderer::{RenderDevice, RenderQueue};

use benilla_assets::materials::WowModelMaterial;
use benilla_assets::{m2_url, quantize, M2Model, WorldAssets};
use benilla_formats::{SeqLoops, UvAnim};
use benilla_ui::script::{ModelPaneFrame, UiScript};
use benilla_ui::widget::{FrameHandle, ModelFileFacts, ModelFog, ModelLight, SequenceFacts};
use benilla_world::doodad_anim::spawn_anim_host;
use benilla_world::lighting::LightBlob;
use benilla_world::mat_anim_table::{affine_row, MatAnimMirrors, MatAnimTable};
use benilla_world::model_forms::ModelForms;
use benilla_world::model_render::M2BatchMaterials;
use benilla_world::particles::buffer::EffectLightOverride;
use benilla_world::particles::{
    spawn_emitter, EmitClock, EmitterFrames, OwnerLoss, ParticleEmitter,
};
use benilla_world::rig_anim::{AnimParked, GlobalSeqDrive, RigPose};
use benilla_world::rig_palette::{RigPaletteMirrors, RigPalettes, RigPart, RigSkin};

use crate::portrait::{
    booth_view_shape, material_variant, new_target_image_sized, pane_projection, StageRig,
    VariantLane, WowPortraitProjection, UI_MODELS_LAYER, UI_MODEL_CAM_LAYERS,
    UI_MODEL_CAM_LAYER_BASE,
};
use crate::ui_pass::{UiQuad, UiQuadAppend, UiQuads, UvRect};

/// `WOW_TILE_TRACE=1`: a `tile-trace:` line per pane per frame from the renderer and the
/// extract's arm, naming the gate a listed pane that draws nothing stopped at.
pub(crate) fn trace_on() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("WOW_TILE_TRACE").as_deref() == Ok("1"));
    *ON
}

/// One pane's tile request, published by the extract's `ModelPane` arm: its device size, the unit
/// ladder `0x76d1a0` derives from it, and the Lua-set scene.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TileRequest {
    /// The `SetModel` path, as written.
    pub path: String,
    /// The pane's rect in whole device pixels: the tile's cell size.
    pub size_px: UVec2,
    /// Device px per model unit: `1280 · modelScale · layoutScale` FrameXML units × seam × DPI.
    pub px_per_unit: f32,
    /// Device px per `SetPosition` unit: `768 · √(a²+1) · layoutScale` FrameXML units × seam × DPI.
    pub pos_px_per_unit: f32,
    /// Device px per model unit of a particle's half-extent, which carries no scale.
    pub star_px_per_unit: f32,
    /// `SetFacing`, radians about the screen normal (CCW positive, the reference's `+Z`).
    pub facing: f32,
    /// `SetPosition`, layout units.
    pub position: Vec3,
    /// The perspective leg's root scale, `G48 · (5/3) · modelScale · layoutScale`.
    pub root_scale: f32,
    /// The perspective leg's root translation: `SetPosition · layoutScale`, model units.
    pub root_pos: Vec3,
    /// The installed camera index (ctor 0, `SetCamera(n)`); `None` is the NULL camera an index
    /// past the file's cameras installs (`0x76cec0`), the orthographic leg of every shipped pane.
    pub camera: Option<u32>,
    /// The pane's embedded `CGLight`, disabled on a fresh `<Model>`, so a lit batch draws black.
    pub light: ModelLight,
    pub fog: Option<ModelFog>,
    /// `ReplaceIconTexture`'s path, the texture of the file's type-14 batches.
    pub icon: Option<String>,
    /// Where the cell composites: the pane's rect in the quad pass's space, y-down logical px.
    pub rect: Rect,
    /// `ZKey::callback(Artwork)`: after every texture and font string of the pane's layer.
    pub z_key: u64,
    /// The frame's own alpha, which the composite draws at (`0x76d120`).
    pub alpha: f32,
    /// The enclosing ScrollFrame's clip, in the quad pass's space.
    pub clip: Option<Rect>,
}

/// Where a tile sits in the atlas, in texels, `y` down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub origin: UVec2,
    pub size: UVec2,
}

/// The extract-to-renderer bridge, reached through `crate::portrait::BoothBridge`.
#[derive(Resource, Default)]
pub(crate) struct UiModelTiles {
    /// The last request per pane. An entry outlives its pane, since the memoized conversion never
    /// says it vanished, so which panes draw is the engine's paint list, never this map.
    pub requests: HashMap<FrameHandle, TileRequest>,
    /// This frame's cell per tile that has something to draw.
    pub cells: HashMap<FrameHandle, Cell>,
    /// The atlas and its size; `None` until the first tile is packed.
    pub atlas: Option<Handle<Image>>,
    pub atlas_size: UVec2,
    /// Device px per logical px, written by the extract and read by its arm to size cells.
    pub dpi: f32,
}

/// A pane's light input: its embedded `CGLight` and its armed fog. Panes with the same scene
/// share one light buffer and twin cache; the default, a disabled light and no fog, is slot 0.
#[derive(Clone, Copy, PartialEq)]
struct TileScene {
    light: ModelLight,
    fog: Option<ModelFog>,
}

impl TileScene {
    /// The `<Model>` ctor's scene, a disabled white light and no fog; every shipped UI M2 is unlit.
    fn default_scene() -> Self {
        Self {
            light: ModelLight::default(),
            fog: None,
        }
    }

    /// The light buffer the reference's collector finalizes for this scene. It gathers only what
    /// `0x76d680` stages, the fog when armed and the light when enabled: a directional light
    /// folds ambient and one diffuse lobe into the SH (`0x71bc70`, `0x71bce0`), a point light
    /// joins the 4-nearest heap with no ambient (`0x71bf90`). The ambient and the lobe land in
    /// probe slot 0, which every tile part's `MeshTag` names; the rig lane reads no rows 0-2.
    fn blob(&self) -> LightBlob {
        let l = self.light;
        let (ambient, lobes, point) = match (l.enabled, l.omni) {
            (false, _) => ([0.0; 3], Vec::new(), None),
            // Type 1, point: model-space position; `1/(0.7d + 0.03d²)` falloff, no range gate.
            (true, true) => (
                [0.0; 3],
                Vec::new(),
                Some((benilla_assets::coords::wow_to_bevy(l.vector), l.diffuse)),
            ),
            // Type 0, directional: `CGLight+0x24` is the direction the light travels. The SH
            // lobe takes it negated (`fchs` at `0x71be7c`, `0x71be81`, `0x71be86` in `0x71bce0`),
            // while the moments at `collector+0x18…+0x50` take it as is.
            (true, false) => (
                l.ambient,
                vec![(
                    (-benilla_assets::coords::wow_to_bevy(l.vector)).normalize_or_zero(),
                    l.diffuse,
                )],
                None,
            ),
        };
        let mut blob = LightBlob::model(ambient, [0.0; 3], Vec3::NEG_Y).probe(ambient, &lobes);
        if let Some((pos, colour)) = point {
            blob = blob.point(pos, POINT_RANGE, colour);
        }
        match self.fog {
            Some(f) => blob.fog_span(f.rgb(), f.near, f.far, true),
            None => blob.fog_span([0.0; 3], 0.0, 0.0, false),
        }
    }
}

/// One slot of the light pool, with the material twins bound to its buffer.
struct TileLight {
    scene: TileScene,
    buffer: Buffer,
    variants: HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
}

/// A point light's candidacy range, unbounded: the reference's 4-nearest gather has none.
const POINT_RANGE: f32 = 1.0e6;

/// The light pool's mirror keys, one per slot. A key is only ever added: the mat-anim upload
/// gates on the mirror count, so a set that grew and shrank in one frame could skip an upload.
const LIGHT_MIRROR_KEYS: [&str; UI_MODEL_CAM_LAYERS] = [
    "ui_models",
    "ui_models_1",
    "ui_models_2",
    "ui_models_3",
    "ui_models_4",
    "ui_models_5",
    "ui_models_6",
    "ui_models_7",
];

/// The ortho tiles' layer and the light pool, where a pane that arms `SetLight(1, …)` or
/// `SetFogColor` takes a slot. A slot costs a whole shared-light buffer, so the pool is capped
/// and a scene past the cap draws as slot 0, the default.
#[derive(Resource)]
struct TileRig {
    layer: RenderLayers,
    lights: Vec<TileLight>,
}

impl TileRig {
    /// The pool slot for `scene`, grown if it is new and there is room, else slot 0.
    fn slot_for(
        &mut self,
        scene: TileScene,
        device: &RenderDevice,
        queue: &RenderQueue,
        mirrors: &mut RigPaletteMirrors,
        anim_mirrors: &mut MatAnimMirrors,
    ) -> usize {
        if let Some(i) = self.lights.iter().position(|l| l.scene == scene) {
            return i;
        }
        if self.lights.len() >= UI_MODEL_CAM_LAYERS {
            warn!("ui_models: the light pool is full — a pane's SetLight/fog is not applied");
            return 0;
        }
        let key = LIGHT_MIRROR_KEYS[self.lights.len()];
        let blob = scene.blob();
        let buffer = blob.create(device, "wow_ui_model_light");
        blob.write(queue, &buffer);
        mirrors.0.insert(key, buffer.clone());
        anim_mirrors.0.insert(key, buffer.clone());
        self.lights.push(TileLight {
            scene,
            buffer,
            variants: HashMap::new(),
        });
        self.lights.len() - 1
    }
}

/// The atlas's orthographic camera, active whenever a cell is packed because it clears the atlas.
#[derive(Component)]
struct TileCamera;

/// A perspective pool camera, aimed each frame at whichever pane holds its `slot`.
#[derive(Component)]
pub(crate) struct TilePerspectiveCamera {
    slot: usize,
}

/// A tile's model root.
#[derive(Component)]
pub(crate) struct TileRoot;

/// A batch whose alpha the file animates, sampled off the pane's play head: a hosted `MatAnim`
/// reads the paused player, which has no node for a sequence that keys no bone, the cooldown's.
struct AlphaPart {
    entity: Entity,
    anim: Arc<benilla_formats::AlphaAnim>,
}

/// A batch whose texture transform animates, with the table rows it writes off the pane's play
/// head; the shader composes them as the reference does, `uv' = R((uv + t − p) ⊙ s) + p`.
struct UvPart {
    /// Held so the clone lives exactly as long as the tile.
    #[allow(dead_code)]
    material: Handle<WowModelMaterial>,
    /// The translation row's slot and its seed (`sun_scale.zw`, the loop's sample at 0).
    trans: Option<(u16, [f32; 2])>,
    /// The affine row's slot: rotation and scale ([`affine_row`]).
    affine: Option<u16>,
    uv_anim: Option<Arc<UvAnim>>,
    uv_seq: Option<Arc<SeqLoops<[f32; 2]>>>,
    uv_rot: Option<Arc<SeqLoops<[f32; 4]>>>,
    uv_scale: Option<Arc<SeqLoops<[f32; 2]>>>,
}

impl UvPart {
    /// Write the translation delta, quantized like the world's lane, and the affine row.
    fn write_rows(
        &self,
        table: &mut MatAnimTable,
        seq_slot: Option<usize>,
        cursor_s: f32,
        gseq_s: f64,
    ) {
        if let Some((slot, seed)) = self.trans {
            let uv = match (&self.uv_seq, &self.uv_anim) {
                (Some(seqs), _) => seqs
                    .seq(seq_slot)
                    .map_or([0.0, 0.0], |l| l.sample(l.clock(cursor_s, gseq_s))),
                (None, Some(a)) => a.sample(a.clock(cursor_s, gseq_s)),
                (None, None) => [0.0, 0.0],
            };
            table.set(
                slot,
                [
                    quantize(uv[0], 4096.0) - seed[0],
                    quantize(uv[1], 4096.0) - seed[1],
                    0.0,
                    0.0,
                ],
            );
        }
        if let Some(slot) = self.affine {
            let q = self
                .uv_rot
                .as_ref()
                .and_then(|r| r.seq(seq_slot))
                .map_or([0.0, 0.0, 0.0, 1.0], |l| {
                    l.sample(l.clock(cursor_s, gseq_s))
                });
            let sc = self
                .uv_scale
                .as_ref()
                .and_then(|r| r.seq(seq_slot))
                .map_or([1.0, 1.0], |l| l.sample(l.clock(cursor_s, gseq_s)));
            table.set(slot, affine_row(q, sc));
        }
    }

    fn free(&self, table: &mut MatAnimTable) {
        if let Some((slot, _)) = self.trans {
            table.free(slot);
        }
        if let Some(slot) = self.affine {
            table.free(slot);
        }
    }
}

/// A live tile: its entity tree and what it was built from. A new file, icon, light slot or
/// camera slot rebuilds it, since a twin binds one light buffer and every entity carries the layer.
struct Tile {
    root: Entity,
    key: String,
    icon: Option<String>,
    m2: Handle<M2Model>,
    /// The tree (parts, rig, emitters) is spawned; until then the root is bare.
    built: bool,
    light_slot: usize,
    /// The perspective slot, and so the render layer, it holds; `None` on the orthographic leg.
    cam_slot: Option<usize>,
    /// The graph node per `AnimationData` id the file keys a bone for, and its file slot.
    clips: HashMap<u16, (AnimationNodeIndex, usize)>,
    /// The id the player has armed, re-armed only on change.
    armed: Option<u16>,
    /// The mat-anim row its materials read their cell clip from (`anim_slots.w`); `None` when the
    /// table was full, and the tile draws unclipped.
    clip_slot: Option<u16>,
    alpha_parts: Vec<AlphaPart>,
    uv_parts: Vec<UvPart>,
    emitters: Vec<Entity>,
    /// The last frame this tile was on the engine's paint list.
    last_seen: u64,
    /// It drew no cell, so its rig and emitters are held.
    parked: bool,
}

impl Tile {
    /// Tear the tile down: its tree, and the table rows its animated materials held.
    fn retire(self, commands: &mut Commands, table: &mut MatAnimTable) {
        for p in &self.uv_parts {
            p.free(table);
        }
        if let Some(slot) = self.clip_slot {
            table.free(slot);
        }
        commands.entity(self.root).despawn();
    }
}

/// Frames a tile survives off the paint list, so a re-arming cooldown or a ping keeps its tree.
const TILE_LINGER_FRAMES: u64 = 600;

/// The atlas edge the first tile allocates, and the cap a grown atlas stops at.
const ATLAS_MIN: u32 = 512;
const ATLAS_MAX: u32 = 4096;

/// Gutter between cells (texels): a tile's bilinear edge never samples a neighbour.
const GUTTER: u32 = 2;

/// The tile camera's order: after every booth (`-100 …`), before the UI camera (`1`).
const TILE_CAMERA_ORDER: isize = -10;

/// The renderer's state beyond the bridge, most of it one VM's: the VM is built at world entry,
/// dropped at the character screen and rebuilt by `ReloadUI()`.
#[derive(Default)]
struct TileState {
    /// The VM these tiles and the bridge's maps belong to ([`UiScript::session`]); 0 for none.
    session: u64,
    tiles: HashMap<FrameHandle, Tile>,
    /// Files the engine asked facts for, loading.
    pending_facts: HashMap<String, Handle<M2Model>>,
    /// Files whose facts are derived, beside the handle that keeps each resident. It records what
    /// the host has loaded, not what a VM has been told: every new VM is still owed the facts.
    loaded: HashMap<String, (Handle<M2Model>, ModelFileFacts)>,
    frame: u64,
}

impl TileState {
    /// Hand the engine the facts it asked for and return the keys still to load. Every ask is
    /// answered, whichever VM asks: `model_facts_wanted` drains and only `SetModel` re-pushes, so
    /// a skipped want keeps that VM's panes on the file off [`UiScript::visible_model_panes`].
    fn answer_facts(&mut self, script: &mut UiScript) -> Vec<String> {
        let mut to_load = Vec::new();
        for key in script.model_facts_wanted() {
            if let Some((_, facts)) = self.loaded.get(&key) {
                if trace_on() {
                    info!("tile-trace: facts for {key} answered from residency");
                }
                script.set_model_facts(&key, facts.clone());
                continue;
            }
            // Already in flight: its landing answers whichever VM is asking by then.
            if self.pending_facts.contains_key(&key) {
                continue;
            }
            to_load.push(key);
        }
        to_load
    }

    /// Adopt the VM `script` names, forgetting every handle-keyed memory for a new one: its arena
    /// reissues the same [`FrameHandle`]s, so a survivor would name a different frame. `None`, no
    /// VM, is session 0.
    fn adopt_vm(
        &mut self,
        script: Option<&UiScript>,
        bridge: &mut UiModelTiles,
        commands: &mut Commands,
        table: &mut MatAnimTable,
    ) {
        let session = script.map_or(0, UiScript::session);
        if self.session == session {
            return;
        }
        self.session = session;
        for (_, tile) in self.tiles.drain() {
            tile.retire(commands, table);
        }
        bridge.requests.clear();
        bridge.cells.clear();
    }
}

pub(crate) struct UiModelsPlugin;

impl Plugin for UiModelsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiModelTiles>()
            .init_non_send_resource::<TileState>()
            .add_systems(Startup, setup_tiles)
            // Ahead of the extract, which publishes the tile requests (`paint_script`).
            .add_systems(
                Update,
                forget_dead_vm_tiles.in_set(crate::ui_script::UiFeed),
            )
            // After the extract publishes this frame's requests, before the pose and palette
            // passes read the roots in PostUpdate.
            .add_systems(
                Update,
                sync_tiles
                    .after(crate::ui_script::UiInput)
                    .after(forget_dead_vm_tiles),
            )
            // The composite, in the minimap fill's lane: after packing, before the mesh rebuild.
            .add_systems(Update, compose_tiles.in_set(UiQuadAppend).after(sync_tiles))
            .add_systems(Update, reap_tile_variants)
            .add_systems(Update, dump_atlas.after(sync_tiles));
    }
}

/// Startup: the tile cameras, the layer and the default black light.
fn setup_tiles(
    mut commands: Commands,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut mirrors: ResMut<RigPaletteMirrors>,
    mut anim_mirrors: ResMut<MatAnimMirrors>,
) {
    let layer = RenderLayers::layer(UI_MODELS_LAYER);
    let mut rig = TileRig {
        layer: layer.clone(),
        lights: Vec::new(),
    };
    // Slot 0, the `<Model>` ctor's scene (`0x76c8e0`). A twin binds its own light buffer, whose
    // palette and mat-anim rows the tiles read, so every pool buffer joins both mirror lists.
    rig.slot_for(
        TileScene::default_scene(),
        &device,
        &queue,
        &mut mirrors,
        &mut anim_mirrors,
    );
    let light_buf = rig.lights[0].buffer.clone();
    commands.spawn((
        Name::new("ui model tiles camera"),
        booth_view_shape(),
        Camera {
            order: TILE_CAMERA_ORDER,
            // The reference clears only depth; a tile composites, so colour clears to transparent.
            clear_color: ClearColorConfig::Custom(Color::NONE),
            is_active: false,
            ..default()
        },
        // Decode, no scene glow: UI models draw after the WorldFrame's FFX apply.
        benilla_world::ffx_glow::FfxGlow::UI_PANE,
        Projection::Orthographic(OrthographicProjection {
            near: 0.1,
            far: 2000.0,
            scaling_mode: ScalingMode::Fixed {
                width: ATLAS_MIN as f32,
                height: ATLAS_MIN as f32,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, 1000.0),
        layer.clone(),
        TileCamera,
    ));
    // The perspective pool, one inactive camera per slot on its own layer, spawned up front: a
    // camera spawned mid-frame misses the render world's extract, so its pane would skip a frame.
    for slot in 0..UI_MODEL_CAM_LAYERS {
        commands.spawn((
            Name::new(format!("ui model pane camera {slot}")),
            booth_view_shape(),
            Camera {
                order: TILE_CAMERA_ORDER + 1 + slot as isize,
                // Load, never clear: the orthographic camera clears the atlas first. Depth
                // clears per camera, the reference's per-widget `GxClear(2)` (`0x76d5e1`).
                clear_color: ClearColorConfig::None,
                is_active: false,
                ..default()
            },
            benilla_world::ffx_glow::FfxGlow::UI_PANE,
            RenderLayers::layer(UI_MODEL_CAM_LAYER_BASE + slot),
            TilePerspectiveCamera { slot },
        ));
    }
    commands.insert_resource(rig);
    let _ = light_buf;
}

/// `pipe_warm`'s orthographic twin of [`setup_tiles`]' camera, kept in its shape: bevy_pbr keys
/// mesh pipelines on the projection class (`bevy_pbr-0.18.1` `render/mesh.rs:397`). The real one
/// is not borrowed, since switching it on would draw the warm models into the live atlas.
pub(crate) fn spawn_warm_tile_cam(
    commands: &mut Commands,
    images: &mut Assets<Image>,
) -> (Entity, RenderLayers) {
    let layer = RenderLayers::layer(crate::portrait::WARM_ORTHO_LAYER);
    // The atlas's minimum size, like the camera it warms; size reaches no pipeline key.
    let image = images.add(new_target_image_sized(ATLAS_MIN, ATLAS_MIN));
    let cam = commands
        .spawn((
            Name::new("pipe_warm orthographic twin camera"),
            booth_view_shape(),
            Camera {
                order: TILE_CAMERA_ORDER - 1,
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            RenderTarget::Image(image.into()),
            benilla_world::ffx_glow::FfxGlow::UI_PANE,
            Projection::Orthographic(OrthographicProjection {
                near: 0.1,
                far: 2000.0,
                scaling_mode: ScalingMode::Fixed {
                    width: ATLAS_MIN as f32,
                    height: ATLAS_MIN as f32,
                },
                ..OrthographicProjection::default_3d()
            }),
            layer.clone(),
        ))
        .id();
    (cam, layer)
}

/// Bevy space to tile-camera space: WoW `+X` right, `+Y` up, `+Z` toward the viewer, the ortho
/// leg's axes (`0x7ad7f0`). A proper rotation, so winding survives.
fn wow_to_screen() -> Quat {
    Quat::from_mat3(&Mat3::from_cols(
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(-1.0, 0.0, 0.0),
    ))
}

/// The assets a tile build reads and writes, in one param under Bevy's 16-parameter ceiling.
#[derive(bevy::ecs::system::SystemParam)]
struct TileAssets<'w> {
    asset_server: Res<'w, AssetServer>,
    m2s: Res<'w, Assets<M2Model>>,
    images: ResMut<'w, Assets<Image>>,
    world: Option<ResMut<'w, WorldAssets>>,
    forms: ResMut<'w, ModelForms>,
    meshes: ResMut<'w, Assets<Mesh>>,
}

/// The render-side resources a tile build spends.
#[derive(bevy::ecs::system::SystemParam)]
struct TileRender<'w> {
    /// The batch materials, and through it the store the twins go in: a second
    /// `ResMut<Assets<WowModelMaterial>>` beside it would conflict.
    mats: M2BatchMaterials<'w>,
    palettes: ResMut<'w, RigPalettes>,
    rig: ResMut<'w, TileRig>,
    /// The shared mat-anim table, where the tiles' materials own rows.
    table: ResMut<'w, MatAnimTable>,
    /// The light pool grows lazily, so the per-frame pass needs the device, queue and mirror lists.
    device: Res<'w, RenderDevice>,
    queue: Res<'w, RenderQueue>,
    mirrors: ResMut<'w, RigPaletteMirrors>,
    anim_mirrors: ResMut<'w, MatAnimMirrors>,
}

/// The VM edge ([`TileState::adopt_vm`]), ordered ahead of the extract: clearing the bridge in
/// [`sync_tiles`] would drop the new VM's first requests, and the memoized extract never re-sends
/// an unchanged one.
fn forget_dead_vm_tiles(
    mut commands: Commands,
    script: Option<NonSend<UiScript>>,
    mut state: NonSendMut<TileState>,
    mut bridge: ResMut<UiModelTiles>,
    mut table: ResMut<MatAnimTable>,
) {
    state.adopt_vm(script.as_deref(), &mut bridge, &mut commands, &mut table);
}

/// The per-frame pass: feed the engine the facts it asked for, keep one tile per visible pane,
/// pack the atlas, place every tile at its cell and its play head, and aim the camera.
#[allow(clippy::type_complexity)] // a Bevy system's full input set
fn sync_tiles(
    mut commands: Commands,
    script: Option<NonSendMut<UiScript>>,
    mut state: NonSendMut<TileState>,
    mut bridge: ResMut<UiModelTiles>,
    mut assets: TileAssets,
    mut render: TileRender,
    mut cams: Query<
        (
            &mut Camera,
            &mut RenderTarget,
            &mut Projection,
            &mut Transform,
        ),
        (With<TileCamera>, Without<TilePerspectiveCamera>),
    >,
    mut pane_cams: Query<
        (
            &mut Camera,
            &mut RenderTarget,
            &mut Projection,
            &mut Transform,
            &TilePerspectiveCamera,
        ),
        Without<TileCamera>,
    >,
    mut roots: Query<
        (
            &mut Transform,
            &mut Visibility,
            Option<&mut AnimationPlayer>,
            Option<&mut GlobalSeqDrive>,
        ),
        (
            With<TileRoot>,
            Without<TileCamera>,
            Without<TilePerspectiveCamera>,
        ),
    >,
    mut parts: Query<
        (&mut MeshTag, &mut Visibility),
        (
            Without<TileRoot>,
            Without<TileCamera>,
            Without<TilePerspectiveCamera>,
        ),
    >,
    mut emitters: Query<&mut ParticleEmitter>,
) {
    state.frame += 1;
    let frame = state.frame;
    let Some(mut script) = script else {
        // No VM: the edge retired the tiles; composite nothing and leave no camera live.
        bridge.cells.clear();
        set_camera_active(&mut cams, false);
        for (mut cam, _, _, _, _) in &mut pane_cams {
            cam.is_active = false;
        }
        return;
    };

    // ── 1. Facts: answered now if the file is resident, else when it lands ───────────────────
    for key in state.answer_facts(&mut script) {
        if trace_on() {
            info!("tile-trace: facts for {key} wanted — loading the file");
        }
        let handle = assets.asset_server.load::<M2Model>(m2_url(&key));
        state.pending_facts.insert(key, handle);
    }
    let landed: Vec<(String, Handle<M2Model>)> = state
        .pending_facts
        .iter()
        .filter(|(_, h)| assets.m2s.contains(*h))
        .map(|(k, h)| (k.clone(), h.clone()))
        .collect();
    for (key, handle) in landed {
        let Some(model) = assets.m2s.get(&handle) else {
            continue; // not readable yet: it stays in `pending_facts`
        };
        let facts = facts_of(model);
        script.set_model_facts(&key, facts.clone());
        state.pending_facts.remove(&key);
        state.loaded.insert(key, (handle, facts));
    }

    // ── 2. The paint list, and one tile per pane on it ──────────────────────────────────
    let panes: Vec<ModelPaneFrame> = script.visible_model_panes();
    let mut live: Vec<(FrameHandle, TileRequest, ModelPaneFrame)> = Vec::new();
    for pane in panes {
        let Some(req) = bridge.requests.get(&pane.handle) else {
            if trace_on() {
                info!(
                    "tile-trace: pane {:?} {} (under {}) on the paint list, not extracted yet",
                    pane.handle,
                    script.frame_name(pane.handle).unwrap_or_default(),
                    script
                        .target_owner_name(benilla_ui::order::ZTarget::Frame(pane.handle))
                        .unwrap_or_default()
                );
            }
            continue; // not extracted yet: next frame
        };
        if req.size_px.x == 0 || req.size_px.y == 0 {
            if trace_on() {
                info!(
                    "tile-trace: {} pane {:?} has a zero rect",
                    req.path, pane.handle
                );
            }
            continue;
        }
        live.push((pane.handle, req.clone(), pane));
    }
    // Cells go in the engine's registry (creation) order, so a pane keeps its cell.
    let _ = &live;

    // ── 2a. Which light rig, and which perspective slot ─────────────────────────────────
    let light_slots: Vec<usize> = live
        .iter()
        .map(|(_, req, _)| {
            render.rig.slot_for(
                TileScene {
                    light: req.light,
                    fog: req.fog,
                },
                &render.device,
                &render.queue,
                &mut render.mirrors,
                &mut render.anim_mirrors,
            )
        })
        .collect();
    // Held slots stay put so no tile rebuilds for its layer; a pane past the pool draws nothing.
    let mut taken = [false; UI_MODEL_CAM_LAYERS];
    let mut cam_slots: Vec<Option<usize>> = live
        .iter()
        .map(|(h, req, _)| {
            let held = state.tiles.get(h).and_then(|t| t.cam_slot);
            match (req.camera.is_some(), held) {
                (true, Some(slot)) if !taken[slot] => {
                    taken[slot] = true;
                    Some(slot)
                }
                (true, _) => None,
                (false, _) => None,
            }
        })
        .collect();
    for (i, (_, req, _)) in live.iter().enumerate() {
        if req.camera.is_none() || cam_slots[i].is_some() {
            continue;
        }
        cam_slots[i] = taken.iter().position(|&t| !t).inspect(|&slot| {
            taken[slot] = true;
        });
    }

    for (i, (handle, req, _)) in live.iter().enumerate() {
        let key = benilla_ui::widget::model_key(&req.path);
        let Some(m2) = state.loaded.get(&key).map(|(h, _)| h.clone()) else {
            if trace_on() {
                info!("tile-trace: {} facts not landed (key {key})", req.path);
            }
            continue; // facts not landed: the engine would not have listed it
        };
        let (light_slot, cam_slot) = (light_slots[i], cam_slots[i]);
        let stale = state.tiles.get(handle).is_some_and(|t| {
            t.key != key
                || t.icon != req.icon
                || t.light_slot != light_slot
                || t.cam_slot != cam_slot
        });
        if stale {
            if let Some(t) = state.tiles.remove(handle) {
                t.retire(&mut commands, &mut render.table);
            }
        }
        let layer = tile_layer(&render.rig, cam_slot);
        let tile = state.tiles.entry(*handle).or_insert_with(|| Tile {
            root: commands
                .spawn((
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    layer.clone(),
                    TileRoot,
                ))
                .id(),
            key: key.clone(),
            icon: req.icon.clone(),
            m2: m2.clone(),
            built: false,
            light_slot,
            cam_slot,
            clips: HashMap::new(),
            armed: None,
            clip_slot: None,
            alpha_parts: Vec::new(),
            uv_parts: Vec::new(),
            emitters: Vec::new(),
            last_seen: frame,
            parked: false,
        });
        tile.last_seen = frame;
        if !tile.built {
            if let Some(model) = assets.m2s.get(&tile.m2) {
                let icon_tex = req.icon.as_deref().and_then(|p| {
                    assets
                        .world
                        .as_mut()
                        .and_then(|w| w.sprite_texture(p, &mut assets.images))
                });
                if let Some(built) = build_tile(
                    &mut commands,
                    tile.root,
                    model,
                    &tile.m2,
                    icon_tex,
                    &mut assets.forms,
                    &mut assets.meshes,
                    &mut render,
                    light_slot,
                    &layer,
                ) {
                    // At debug, by pane: five cooldowns build five tiles, and one longer
                    // than the linger rebuilds at combat rate.
                    debug!(
                        "ui_models: tile built for {} on pane {} — {} parts, {} emitters, \
                         {} animated alphas",
                        req.path,
                        script.frame_name(*handle).unwrap_or_default(),
                        model.submeshes.len(),
                        built.emitters.len(),
                        built.alpha_parts.len()
                    );
                    if trace_on() {
                        for (i, p) in built.uv_parts.iter().enumerate() {
                            info!(
                                "tile-trace: {} uv part {i}: trans slot {:?} affine slot {:?}",
                                req.path,
                                p.trans.map(|(s, _)| s),
                                p.affine
                            );
                        }
                    }
                    tile.clips = built.clips;
                    tile.clip_slot = built.clip_slot;
                    tile.alpha_parts = built.alpha_parts;
                    tile.uv_parts = built.uv_parts;
                    tile.emitters = built.emitters;
                    tile.built = true;
                } else if trace_on() {
                    info!("tile-trace: {} waiting on materials", req.path);
                }
            } else if trace_on() {
                info!("tile-trace: {} asset not resident", req.path);
            }
        }
    }

    // ── 3. Retire tiles that left the paint list long ago ───────────────────────────────
    let dead: Vec<FrameHandle> = state
        .tiles
        .iter()
        .filter(|(_, t)| frame.saturating_sub(t.last_seen) > TILE_LINGER_FRAMES)
        .map(|(h, _)| *h)
        .collect();
    for h in dead {
        if let Some(t) = state.tiles.remove(&h) {
            t.retire(&mut commands, &mut render.table);
        }
        bridge.requests.remove(&h);
    }

    // ── 4. Pack the atlas ───────────────────────────────────────────────────────────────
    let drawing: Vec<&(FrameHandle, TileRequest, ModelPaneFrame)> = live
        .iter()
        .filter(|(h, r, _)| {
            state.tiles.get(h).is_some_and(|t| {
                // A perspective pane with no free camera draws nothing: the orthographic
                // ladder would draw it far too large.
                t.built && !(r.camera.is_some() && t.cam_slot.is_none())
            })
        })
        .collect();
    let sizes: Vec<UVec2> = drawing.iter().map(|(_, r, _)| r.size_px).collect();
    let (cells, atlas_size) = pack(&sizes);
    if atlas_size != bridge.atlas_size || bridge.atlas.is_none() {
        if atlas_size.x > 0 {
            let image = assets
                .images
                .add(new_target_image_sized(atlas_size.x, atlas_size.y));
            for (_, mut target, mut proj, mut tf) in &mut cams {
                *target = RenderTarget::Image(image.clone().into());
                *proj = Projection::Orthographic(OrthographicProjection {
                    near: 0.1,
                    far: 2000.0,
                    scaling_mode: ScalingMode::Fixed {
                        width: atlas_size.x as f32,
                        height: atlas_size.y as f32,
                    },
                    ..OrthographicProjection::default_3d()
                });
                // The camera looks down `−Z` at the atlas plane, centred; a tile at world
                // `(x, y)` lands at texel `(x, H − y)`.
                *tf = Transform::from_xyz(
                    atlas_size.x as f32 * 0.5,
                    atlas_size.y as f32 * 0.5,
                    1000.0,
                );
            }
            bridge.atlas = Some(image);
        }
        bridge.atlas_size = atlas_size;
    }
    bridge.cells.clear();
    let atlas_h = atlas_size.y as f32;

    // ── 5. Place every drawing tile: cell, unit ladder, facing, play head ───────────────
    let mut aimed = [false; UI_MODEL_CAM_LAYERS];
    for (i, (handle, req, pane)) in drawing.iter().enumerate() {
        let Some(cell) = cells.get(i).copied() else {
            continue; // did not fit the capped atlas
        };
        let Some(tile) = state.tiles.get_mut(handle) else {
            continue;
        };
        bridge.cells.insert(*handle, cell);
        let Ok((mut tf, mut vis, player, drive)) = roots.get_mut(tile.root) else {
            continue;
        };

        let (armed, cursor_s, seq_slot) = match pane.play {
            Some(ph) => {
                let slot = tile.clips.get(&ph.anim_id).map(|&(_, s)| s);
                (Some(ph.anim_id), ph.cursor_ms as f32 / 1000.0, slot)
            }
            None => (None, 0.0, None),
        };

        // ── The leg ────────────────────────────────────────────────────────────────────
        // Perspective (`0x7ac640`): the file's camera frames the pane from this tile's camera,
        // viewport the cell, and the eye and target ride the model's root (`0x718960`).
        // Orthographic: one model unit is `1280 · modelScale · layoutScale` FrameXML units.
        let perspective = tile.cam_slot.zip(req.camera).and_then(|(slot, idx)| {
            let cam = assets.m2s.get(&tile.m2)?.cameras.get(idx as usize)?;
            Some((slot, cam.clone()))
        });
        let mut leg_trace = String::from("ortho");
        if let Some((slot, cam)) = perspective {
            // Camera tracks read the file's absolute timeline and the play head is a cursor in
            // the armed band, so the band's start is added.
            let file_ms = seq_slot
                .and_then(|slot| assets.m2s.get(&tile.m2)?.sequences.get(slot))
                .map_or(0, |seq| seq.start_ms)
                + pane.play.map_or(0, |ph| ph.cursor_ms);
            let record = cam.at(file_ms);
            let aspect = req.size_px.x as f32 / req.size_px.y.max(1) as f32;
            let leg = perspective_rig(&record, req, aspect);
            *tf = leg.root;
            *vis = Visibility::Visible;
            for (mut c, mut target_ref, mut proj, mut ctf, marker) in &mut pane_cams {
                if marker.slot != slot {
                    continue;
                }
                // The target first, and active only with one: the default target is the
                // window, so an active camera without the atlas would draw over the game.
                let Some(atlas) = bridge.atlas.clone() else {
                    continue;
                };
                *target_ref = RenderTarget::Image(atlas.into());
                aimed[slot] = true;
                c.is_active = true;
                c.viewport = Some(bevy::camera::Viewport {
                    physical_position: cell.origin,
                    physical_size: cell.size,
                    depth: 0.0..1.0,
                });
                *proj = Projection::custom(leg.projection.clone());
                *ctf = leg.camera;
            }
            if trace_on() {
                use bevy::camera::CameraProjection;
                let m = leg.projection.get_clip_from_view();
                let eye = leg.camera.translation;
                let fwd = leg.camera.forward().as_vec3();
                leg_trace = format!(
                    "persp[cam {:?} slot {slot} t={file_ms}ms] fov={:.5} aspect={aspect:.4} near={:.4} far={:.3} eye={:.3},{:.3},{:.3} fwd={:.3},{:.3},{:.3} root(s={:.4} pos={:?}) m00={:.4} m11={:.4}",
                    req.camera,
                    record.fov,
                    record.near,
                    record.far,
                    eye.x,
                    eye.y,
                    eye.z,
                    fwd.x,
                    fwd.y,
                    fwd.z,
                    req.root_scale,
                    req.root_pos.to_array(),
                    m.x_axis.x,
                    m.y_axis.y,
                );
            }
        } else {
            // The root: the cell's bottom-left plus `SetPosition` (layout units), in camera space.
            let cell_bl = Vec2::new(
                cell.origin.x as f32,
                atlas_h - (cell.origin.y + cell.size.y) as f32,
            );
            let pos = Vec2::new(req.position.x, req.position.y) * req.pos_px_per_unit;
            let depth = req.position.z * req.pos_px_per_unit;
            // `T(pos) · R(facing about WoW +Z) · S(px per unit)`: the facing turns about bevy
            // `+Y` (WoW's `+Z`), then the axis fix, then the scale.
            *tf = Transform {
                translation: Vec3::new(cell_bl.x + pos.x, cell_bl.y + pos.y, depth),
                rotation: wow_to_screen() * Quat::from_rotation_y(req.facing),
                scale: Vec3::splat(req.px_per_unit),
            };
            *vis = Visibility::Visible;
        }
        if let Some(mut player) = player {
            if tile.armed != armed {
                player.stop_all();
                if let Some((node, _)) = armed.and_then(|id| tile.clips.get(&id)) {
                    player.play(*node).pause();
                }
                tile.armed = armed;
            }
            if let Some((node, _)) = armed.and_then(|id| tile.clips.get(&id)) {
                if let Some(active) = player.animation_mut(*node) {
                    active.seek_to(cursor_s);
                }
            }
        }
        // The file slot the material tracks read: the armed clip's, or, when the armed id keys
        // no bone (the cooldown), the file's first sequence with that id.
        let seq_slot = seq_slot.or_else(|| {
            let id = armed?;
            let facts_slot = assets
                .m2s
                .get(&tile.m2)?
                .sequences
                .iter()
                .find(|s| s.anim_id == id)
                .map(|s| s.seq_index);
            facts_slot
        });
        let gseq_s = pane.clock_ms as f64 / 1000.0;
        // The bone global sequences read the pane's clock too: the kernel's cursor,
        // `[[model+0x2c]+0xc] − [model+0x68]` (`0x714260`), is the owning scene's clock minus the
        // attach snapshot, and a `<Model>` owns a private `CM2Scene` (`CSimpleModel+0x314`) that
        // only its `OnUpdate` advances (`0x76d7f0`). So the ping's 4833 ms spinner resumes where
        // the last ping stopped; the drive anchors on its first tick, the attach.
        if let Some(mut d) = drive {
            d.set_clock(gseq_s);
        }
        let light_trace = if trace_on() {
            let sc = &render.rig.lights[tile.light_slot].scene;
            format!(
                "slot{} enabled={} omni={} fog={:?}",
                tile.light_slot, sc.light.enabled, sc.light.omni, sc.fog
            )
        } else {
            String::new()
        };
        let mut trace_alphas: Vec<f32> = Vec::new();
        for part in &tile.alpha_parts {
            let a = part.anim.sample(seq_slot, cursor_s, gseq_s);
            if trace_on() {
                trace_alphas.push(a);
            }
            if let Ok((mut tag, mut pvis)) = parts.get_mut(part.entity) {
                // The `A ≤ 0` cull (`0x707b3a`): a batch keyed off in this sequence is skipped.
                let want = if a > 0.0 {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
                if *pvis != want {
                    *pvis = want;
                }
                let bits = benilla_world::mesh_tag::with_alpha(tag.0, a);
                if tag.0 != bits {
                    tag.0 = bits;
                }
            }
        }
        for part in &tile.uv_parts {
            part.write_rows(&mut render.table, seq_slot, cursor_s, gseq_s);
        }
        if trace_on() {
            let rows: Vec<String> = tile
                .uv_parts
                .iter()
                .map(|p| {
                    let t = p.trans.map(|(s, _)| render.table.row(s));
                    let a = p.affine.map(|s| render.table.row(s));
                    format!("t={t:?} a={a:?}")
                })
                .collect();
            info!(
                "tile-trace: {} {} leg={} cell=({},{} {}x{}) px/unit={:.2} pos_px/unit={:.2} armed={:?} cursor={:.3}s slot={:?} clock={}ms light={light_trace} alphas={:?} rows=[{}]",
                req.path,
                script.frame_name(*handle).unwrap_or_default(),
                leg_trace,
                cell.origin.x,
                cell.origin.y,
                cell.size.x,
                cell.size.y,
                req.px_per_unit,
                req.pos_px_per_unit,
                armed,
                cursor_s,
                seq_slot,
                pane.clock_ms,
                trace_alphas,
                rows.join(", ")
            );
        }
        // A particle's half-extent is in eye space: the ortho leg's is the cell's pixels at
        // `768·√(a²+1)` FrameXML units per model unit (`0x7b2a50`); the perspective leg's is 1.0.
        let star = if tile.cam_slot.is_some() && req.camera.is_some() {
            1.0
        } else {
            req.star_px_per_unit
        };
        // The cell the cloud may draw in, or it spills into its packed neighbour; `Cell::origin`
        // is atlas texels, top-left, the framebuffer coordinate the fragment tests.
        let clip = Vec4::new(
            cell.origin.x as f32,
            cell.origin.y as f32,
            (cell.origin.x + cell.size.x) as f32,
            (cell.origin.y + cell.size.y) as f32,
        );
        // The mesh half of the clip: the tile's row, read through `anim_slots.w`.
        if let Some(slot) = tile.clip_slot {
            render.table.set(slot, clip.to_array());
        }
        for &e in &tile.emitters {
            if let Ok(mut em) = emitters.get_mut(e) {
                em.set_size_scale(star);
                em.set_clip(Some(clip));
            }
        }
    }
    // ── 6. Park what is not drawing; thaw what is ──────────────────────────────────────
    // Drawing means holding a cell. Hiding the root stops neither the emitters nor the bone
    // global sequences, so `AnimParked` holds the rig and the freeze holds each emitter.
    for (handle, tile) in state.tiles.iter_mut() {
        let park = !draws_this_frame(&bridge, handle);
        // Compared, not edge-triggered off `tile.parked`: a tile can be built already parked,
        // and an emitter is born thawed. Reading through `Deref` touches no change tick.
        for &e in &tile.emitters {
            if let Ok(mut em) = emitters.get_mut(e) {
                if em.is_frozen() != park {
                    em.set_frozen(park);
                }
            }
        }
        if tile.parked == park {
            continue;
        }
        tile.parked = park;
        if trace_on() {
            info!(
                "tile-trace: tile {:?} {} — {} emitters",
                handle,
                if park { "PARKED" } else { "thawed" },
                tile.emitters.len()
            );
        }
        if park {
            if let Ok((_, mut vis, _, _)) = roots.get_mut(tile.root) {
                *vis = Visibility::Hidden;
            }
            commands.entity(tile.root).insert(AnimParked);
        } else {
            // Removed in `Update`, ahead of `AnimationSystems` in `PostUpdate`, so the first
            // thawed frame evaluates the pose before anything reads it.
            commands.entity(tile.root).remove::<AnimParked>();
        }
    }
    for (mut cam, _, _, _, marker) in &mut pane_cams {
        let want = aimed[marker.slot];
        if cam.is_active != want {
            cam.is_active = want;
        }
    }
    // The orthographic camera clears the atlas for all, so it stays on while anything is packed.
    set_camera_active(
        &mut cams,
        !bridge.cells.is_empty() && bridge.atlas.is_some(),
    );
}

/// The perspective leg's rig, pure so the reference's worked numbers test without a world.
struct PerspectiveRig {
    /// The model's root, `T(pos · layoutScale) · R(facing, +Z) · S(s)` in Bevy model space:
    /// `0x76d1a0`'s `model+0xbc` without the orthographic leg's pixel ladder.
    root: Transform,
    /// The camera, `lookAt(eye, target, up)`, with the eye as the view origin.
    camera: Transform,
    /// The record's projection at the pane's own width/height.
    projection: WowPortraitProjection,
}

/// Build the rig. The authored eye and target ride the root transform (`0x718960` publishes
/// `eye = (position_base + posTrack) · M_root`), so `SetModelScale` and `SetPosition` cancel for
/// framing. Scale still matters: near and far are copied unscaled (`0x70ebd0`) while eye depth
/// scales with `s`, so a large scale pushes it through the far plane, a small one the near.
///
/// `0x7ac640` builds up from `CCamera` fields the publish never writes, as
/// `(sin(a₆)·sin(roll), −cos(a₆)·sin(roll), cos(roll))`; `a₆` has no writer and stays 0, so at
/// roll 0 up is model-space `+Z`, the axis `SetFacing` turns about, and the facing cancels too.
///
/// Deviation: the reference's paint (`0x76d240`) reads the eye before it rebuilds the root, so a
/// facing change draws one frame late; not reproduced, because it is an ordering quirk.
fn perspective_rig(
    record: &benilla_assets::PortraitCamera,
    req: &TileRequest,
    aspect: f32,
) -> PerspectiveRig {
    let root = Transform {
        translation: benilla_assets::coords::wow_to_bevy(req.root_pos.to_array()),
        rotation: Quat::from_rotation_y(req.facing),
        scale: Vec3::splat(req.root_scale),
    };
    let m = root.to_matrix();
    let (eye, target) = (
        m.transform_point3(record.eye),
        m.transform_point3(record.target),
    );
    // The camera's own up, not a roll about the view axis; the two agree at roll 0.
    let (sin_roll, cos_roll) = record.roll.sin_cos();
    let up = benilla_assets::coords::wow_to_bevy([0.0, -sin_roll, cos_roll]);
    PerspectiveRig {
        root,
        camera: Transform::from_translation(eye).looking_at(target, up),
        projection: pane_projection(record, aspect),
    }
}

/// Drop twin-cache entries whose world material died, as the booths do: the cache alone pins a
/// twin, which would outlive a map teardown; a live tile's `MeshMaterial3d` still holds its own.
fn reap_tile_variants(
    mut events: MessageReader<AssetEvent<WowModelMaterial>>,
    rig: Option<ResMut<TileRig>>,
) {
    let Some(mut rig) = rig else { return };
    for ev in events.read() {
        if let AssetEvent::Removed { id } = ev {
            for light in &mut rig.lights {
                light.variants.remove(id);
            }
        }
    }
}

/// A tile's render layer: the atlas camera's for an orthographic tile, its own camera's for a
/// perspective one, or every perspective camera would draw every other pane's model.
fn tile_layer(rig: &TileRig, cam_slot: Option<usize>) -> RenderLayers {
    match cam_slot {
        Some(slot) => RenderLayers::layer(UI_MODEL_CAM_LAYER_BASE + slot),
        None => rig.layer.clone(),
    }
}

/// `WOW_TILE_DUMP=<path>:<secs>`: save the tile atlas once, `secs` of app time in; an
/// all-transparent dump means no pane was drawing.
fn dump_atlas(
    mut commands: Commands,
    bridge: Res<UiModelTiles>,
    time: Res<Time<bevy::time::Real>>,
    mut done: Local<bool>,
) {
    static SPEC: std::sync::OnceLock<Option<(String, f32)>> = std::sync::OnceLock::new();
    let Some((path, secs)) = SPEC.get_or_init(|| {
        let v = std::env::var("WOW_TILE_DUMP").ok()?;
        let (path, secs) = v.rsplit_once(':')?;
        Some((path.to_string(), secs.parse().ok()?))
    }) else {
        return;
    };
    if *done || time.elapsed_secs() < *secs {
        return;
    }
    let Some(atlas) = bridge.atlas.clone() else {
        return; // no atlas yet: wait for the first tile
    };
    *done = true;
    use bevy::render::view::window::screenshot::{Screenshot, ScreenshotCaptured};
    info!(
        "WOW_TILE_DUMP: shooting the {}x{} tile atlas ({} cell(s)) -> {path}",
        bridge.atlas_size.x,
        bridge.atlas_size.y,
        bridge.cells.len()
    );
    let out = std::path::PathBuf::from(path.clone());
    commands
        .spawn(Screenshot::image(atlas))
        .observe(move |shot: On<ScreenshotCaptured>| {
            let Some(img) = crate::portrait::test_bake::encode_target_readback(&shot.image) else {
                warn!("WOW_TILE_DUMP: unexpected target format, nothing saved");
                return;
            };
            match img.try_into_dynamic() {
                Ok(dyn_img) => match dyn_img.save(&out) {
                    Ok(()) => info!("WOW_TILE_DUMP: saved {}", out.display()),
                    Err(e) => warn!("WOW_TILE_DUMP: save failed: {e}"),
                },
                Err(e) => warn!("WOW_TILE_DUMP: convert failed: {e}"),
            }
        });
}

#[allow(clippy::type_complexity)] // the system's own query, borrowed
fn set_camera_active(
    cams: &mut Query<
        (
            &mut Camera,
            &mut RenderTarget,
            &mut Projection,
            &mut Transform,
        ),
        (With<TileCamera>, Without<TilePerspectiveCamera>),
    >,
    active: bool,
) {
    for (mut cam, _, _, _) in cams {
        if cam.is_active != active {
            cam.is_active = active;
        }
    }
}

/// The composite, a premultiplied quad per cell packed this frame: drawn here, not by the
/// extract, whose memoized conversion would not re-run when a cell lands.
pub(crate) fn compose_tiles(bridge: Res<UiModelTiles>, mut quads: ResMut<UiQuads>) {
    quads.overlays.extend(composite_quads(&bridge));
}

/// Whether a tile draws this frame, the park verdict inverted: it holds a cell, the set
/// [`composite_quads`] draws. [`sync_tiles`] inserts each placed pane's cell before any
/// `continue`, and a pane the capped atlas could not fit has none, so no drawing pane parks.
fn draws_this_frame(bridge: &UiModelTiles, handle: &FrameHandle) -> bool {
    bridge.cells.contains_key(handle)
}

/// [`compose_tiles`]'s pure half, sorted by paint key so the overlay diff sees a stable order.
pub(crate) fn composite_quads(bridge: &UiModelTiles) -> Vec<UiQuad> {
    let Some(atlas) = bridge.atlas.clone() else {
        return Vec::new();
    };
    let a = bridge.atlas_size.as_vec2();
    if a.x <= 0.0 || a.y <= 0.0 {
        return Vec::new();
    }
    let mut out: Vec<UiQuad> = bridge
        .cells
        .iter()
        .filter_map(|(handle, cell)| {
            let req = bridge.requests.get(handle)?;
            let (u0, v0) = (cell.origin.x as f32 / a.x, cell.origin.y as f32 / a.y);
            let (u1, v1) = (
                (cell.origin.x + cell.size.x) as f32 / a.x,
                (cell.origin.y + cell.size.y) as f32 / a.y,
            );
            Some(UiQuad {
                rect: req.rect,
                z_key: req.z_key,
                texture: Some(atlas.clone()),
                uv: UvRect::from_tex_coords([u0, u1, v0, v1]),
                // The widget's own alpha (`0x76d120`).
                color: [1.0, 1.0, 1.0, req.alpha],
                // A render target is premultiplied.
                premultiplied: true,
                clip: req.clip,
                ..default()
            })
        })
        .collect();
    out.sort_by_key(|q| q.z_key);
    out
}

/// The engine's facts for a resident file: its sequences, header bounds and camera count.
fn facts_of(model: &M2Model) -> ModelFileFacts {
    ModelFileFacts {
        sequences: model
            .sequences
            .iter()
            .map(|s| SequenceFacts {
                anim_id: s.anim_id,
                duration_ms: s.duration_ms,
                looping: s.looping,
            })
            .collect(),
        bbox: model
            .bounds
            .as_ref()
            .map_or(([0.0; 3], [0.0; 3]), |b| (b.bbox_min, b.bbox_max)),
        cameras: model.cameras.len() as u32,
    }
}

/// Shelf-pack `sizes` into the smallest power-of-two square atlas from [`ATLAS_MIN`] to
/// [`ATLAS_MAX`] that fits: the cells that fit, in order, and the atlas size (`0×0` for none).
fn pack(sizes: &[UVec2]) -> (Vec<Cell>, UVec2) {
    if sizes.is_empty() {
        return (Vec::new(), UVec2::ZERO);
    }
    let mut edge = ATLAS_MIN;
    loop {
        let cells = shelf_pack(sizes, edge);
        if cells.len() == sizes.len() || edge >= ATLAS_MAX {
            return (cells, UVec2::splat(edge));
        }
        edge *= 2;
    }
}

/// Rows of cells left to right, a new row when one would overflow; stops at the first size
/// that cannot fit the remaining height (so the returned cells are a prefix of `sizes`).
fn shelf_pack(sizes: &[UVec2], edge: u32) -> Vec<Cell> {
    let mut cells = Vec::with_capacity(sizes.len());
    let (mut x, mut y, mut row_h) = (GUTTER, GUTTER, 0u32);
    for &size in sizes {
        let (w, h) = (size.x, size.y);
        if w + 2 * GUTTER > edge || h + 2 * GUTTER > edge {
            break;
        }
        if x + w + GUTTER > edge {
            x = GUTTER;
            y += row_h + GUTTER;
            row_h = 0;
        }
        if y + h + GUTTER > edge {
            break;
        }
        cells.push(Cell {
            origin: UVec2::new(x, y),
            size,
        });
        x += w + GUTTER;
        row_h = row_h.max(h);
    }
    cells
}

/// What [`build_tile`] made.
struct BuiltTile {
    clips: HashMap<u16, (AnimationNodeIndex, usize)>,
    clip_slot: Option<u16>,
    alpha_parts: Vec<AlphaPart>,
    uv_parts: Vec<UvPart>,
    emitters: Vec<Entity>,
}

/// Spawn a file's parts, rig and emitters under `root`, the booth bake's recipe for a file with
/// no unit. `None` until the batch materials are ready; the caller retries next frame.
fn build_tile(
    commands: &mut Commands,
    root: Entity,
    model: &M2Model,
    handle: &Handle<M2Model>,
    icon_tex: Option<Handle<Image>>,
    forms: &mut ModelForms,
    meshes: &mut Assets<Mesh>,
    render: &mut TileRender,
    light_slot: usize,
    layer: &RenderLayers,
) -> Option<BuiltTile> {
    if !render.mats.ready() {
        return None;
    }
    let light = render.rig.lights.get(light_slot)?.buffer.clone();
    let fogged = render.rig.lights[light_slot].scene.fog.is_some();
    let layer = layer.clone();
    // The render forms now, unpaced: one small model, on demand.
    forms.ensure_now_rigged(handle, &model.submeshes, meshes);
    let built = forms.slices(handle);
    let (stat_forms, skin_forms) = (built.stat, built.skin.unwrap_or(&[]));

    // The cell-clip row, `anim_slots.w` on every material: the clip is per pane and a twin per
    // (material, light), so every batch draws through a clone of its own.
    let clip_slot = render.table.alloc();
    // Materials first, so a `None` return leaves no half-built tree.
    let mut part_mats: Vec<Handle<WowModelMaterial>> = Vec::with_capacity(model.submeshes.len());
    let mut uv_parts: Vec<UvPart> = Vec::new();
    for (i, sub) in model.submeshes.iter().enumerate() {
        let texture = if sub.icon_slot {
            icon_tex.clone()
        } else {
            sub.texture.clone()
        };
        let world = render.mats.steady(sub, texture, (i + 1) as u16)?;
        // The twin: this rig's light buffer, and when the pane armed fog the batch's authored
        // fog policy, so an `UNFOGGED` material stays unfogged (`0x70bb24`); no fog forces it off.
        let lane = if fogged {
            VariantLane::RigFogged
        } else {
            VariantLane::RigUnfogged
        };
        let twin = material_variant(
            &mut render.rig.lights[light_slot].variants,
            &light,
            &world,
            render.mats.materials(),
            lane,
        )?;
        // An animated texture transform also gets its own table rows, off this pane's play head.
        let animated = sub.uv_anim.is_some()
            || sub.uv_seq.is_some()
            || sub.uv_rot_seq.is_some()
            || sub.uv_scale_seq.is_some();
        {
            let mut own = render.mats.materials().get(&twin).cloned()?;
            own.extension.anim_slots.w = clip_slot.map_or(0.0, f32::from);
            let seed = [own.extension.sun_scale.z, own.extension.sun_scale.w];
            let trans = (animated && (sub.uv_anim.is_some() || sub.uv_seq.is_some()))
                .then(|| render.table.alloc())
                .flatten()
                .map(|slot| {
                    own.extension.anim_slots.x = f32::from(slot);
                    (slot, seed)
                });
            let affine = (animated && (sub.uv_rot_seq.is_some() || sub.uv_scale_seq.is_some()))
                .then(|| render.table.alloc())
                .flatten()
                .inspect(|&slot| own.extension.anim_slots.z = f32::from(slot));
            let handle = render.mats.materials().add(own);
            if animated {
                uv_parts.push(UvPart {
                    material: handle.clone(),
                    trans,
                    affine,
                    uv_anim: sub.uv_anim.clone(),
                    uv_seq: sub.uv_seq.clone(),
                    uv_rot: sub.uv_rot_seq.clone(),
                    uv_scale: sub.uv_scale_seq.clone(),
                });
            }
            part_mats.push(handle);
        }
    }

    // The rig, when the file has bones: the collapsed pose and a palette slot. Bone billboards
    // are not applied, since the tile camera is not the world's; no shipped UI file has one.
    let mut pose: Option<RigPose> = None;
    let mut slot: u16 = 0;
    let mut clips: HashMap<u16, (AnimationNodeIndex, usize)> = HashMap::new();
    if !model.skeleton.joints.is_empty() {
        let p = RigPose::new(root, &model.skeleton).without_camera_billboards();
        slot = RigSkin::allocate_bones(
            &mut render.palettes,
            model.skeleton.joints.len() as u32,
            model.inverse_bindposes.clone(),
        )
        .map_or(0, |rig| {
            let s = rig.slot;
            commands.entity(root).insert(rig);
            render.palettes.mark_mirrored(s);
            s
        });
        if let Some(anims) = &model.animations {
            for c in &anims.clips {
                clips.entry(c.anim_id).or_insert((c.node, c.seq_index));
            }
            // A paused player, seeked every frame to the engine's play head.
            let mut player = AnimationPlayer::default();
            player.stop_all();
            commands.entity(root).insert((
                player,
                AnimationGraphHandle(anims.graph.clone()),
                anims.clone(),
            ));
            if let Some(drive) = GlobalSeqDrive::new_rig(&anims.global_bones, p.locals.len()) {
                commands.entity(root).insert(drive);
            }
        }
        pose = Some(p);
    }

    // The parts.
    let mut alpha_parts = Vec::new();
    for (i, sub) in model.submeshes.iter().enumerate() {
        let use_rig = slot != 0 && skin_forms.get(i).is_some();
        let mesh = if use_rig {
            skin_forms[i].clone()
        } else {
            stat_forms
                .get(i)
                .map(|(h, _)| h.clone())
                .unwrap_or_default()
        };
        let tag_slot = if use_rig { slot } else { 0 };
        let mut child = commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(part_mats[i].clone()),
            MeshTag(benilla_world::mesh_tag::spawn_tag(tag_slot, 1.0)),
            Transform::IDENTITY,
            layer.clone(),
            ChildOf(root),
            // Skinned or not, a tile's part is framed by construction; there is no cull to lose.
            NoFrustumCulling,
        ));
        if use_rig {
            child.insert(RigPart(root));
        }
        let entity = child.id();
        if let Some(anim) = &sub.alpha_anim {
            alpha_parts.push(AlphaPart {
                entity,
                anim: anim.clone(),
            });
        }
    }

    // The emitters: on their bone's anchor, or the root for a boneless file; clocked by the
    // root's player, sized per frame, lit by the tile's buffer but not fogged by it (the effect
    // lane reads its own fog uniform), so a fogged pane's cloud draws unfogged.
    let mut emitters = Vec::new();
    for em in &model.emitters {
        let (owner, pivot) = match pose.as_mut() {
            Some(p) => p
                .anchor_for(commands, root, em.def.bone)
                .map_or((root, [0.0; 3]), |joint| (joint, em.bone_pivot)),
            None => (root, [0.0; 3]),
        };
        let Some(e) = spawn_emitter(
            commands,
            em,
            Transform::IDENTITY,
            EmitterFrames {
                owner: Some((owner, pivot)),
                anchor: Some(root),
                alpha: None,
                light_node: None,
                on_owner_loss: OwnerLoss::Free,
            },
            EmitClock::Host(root),
        ) else {
            continue;
        };
        commands.entity(e).insert((
            layer.clone(),
            ChildOf(root),
            EffectLightOverride(light.clone()),
        ));
        emitters.push(e);
    }

    if let Some(p) = pose {
        commands.entity(root).insert((p, StageRig));
    }
    // Not `spawn_anim_host`, the world's placement recipe: a widget arms exactly what Lua asked.
    let _ = spawn_anim_host;
    Some(BuiltTile {
        clips,
        clip_slot,
        alpha_parts,
        uv_parts,
        emitters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_packer_keeps_cells_apart_and_in_order() {
        let sizes: Vec<UVec2> = (0..40).map(|_| UVec2::new(72, 72)).collect();
        let (cells, atlas) = pack(&sizes);
        assert_eq!(cells.len(), 40);
        // 6 per row at 512 (74 px each with the gutter) is 36; the 40th needs the next size.
        assert_eq!(atlas, UVec2::splat(1024));
        for (i, a) in cells.iter().enumerate() {
            assert!(a.origin.x + a.size.x <= atlas.x && a.origin.y + a.size.y <= atlas.y);
            for b in &cells[i + 1..] {
                let apart = a.origin.x + a.size.x + GUTTER <= b.origin.x
                    || b.origin.x + b.size.x + GUTTER <= a.origin.x
                    || a.origin.y + a.size.y + GUTTER <= b.origin.y
                    || b.origin.y + b.size.y + GUTTER <= a.origin.y;
                assert!(apart, "cells {i} and another overlap or touch");
            }
        }
        let big: Vec<UVec2> = (0..8).map(|_| UVec2::new(400, 400)).collect();
        let (cells, atlas) = pack(&big);
        assert_eq!(cells.len(), 8);
        assert_eq!(atlas, UVec2::splat(2048));
        let huge = vec![UVec2::new(5000, 10)];
        let (cells, atlas) = pack(&huge);
        assert!(cells.is_empty());
        assert_eq!(atlas, UVec2::splat(ATLAS_MAX));
    }

    #[test]
    fn the_composite_is_the_bridges_cells_over_their_requests() {
        // Two real handles; the bridge only keys by them.
        let mut arena = benilla_ui::widget::WidgetArena::new();
        let handle = arena.create(benilla_ui::widget::FrameKind::Frame, None, None);
        let stray = arena.create(benilla_ui::widget::FrameKind::Frame, None, None);
        let req = TileRequest {
            path: r"Interface\Cooldown\UI-Cooldown-Indicator.mdx".into(),
            size_px: UVec2::new(63, 63),
            px_per_unit: 1680.75,
            pos_px_per_unit: 2742.62,
            star_px_per_unit: 2742.62,
            facing: 0.0,
            position: Vec3::ZERO,
            root_scale: 1.0,
            root_pos: Vec3::ZERO,
            camera: None,
            light: ModelLight::default(),
            fog: None,
            icon: None,
            rect: Rect::new(303.2, 767.1, 334.9, 798.8),
            z_key: 3_458_840_389_530_157_056,
            alpha: 0.5,
            clip: Some(Rect::new(0.0, 700.0, 400.0, 800.0)),
        };
        let mut bridge = UiModelTiles::default();
        bridge.requests.insert(handle, req.clone());
        assert!(
            composite_quads(&bridge).is_empty(),
            "no atlas, no cell: nothing"
        );
        bridge.atlas = Some(Handle::default());
        bridge.atlas_size = UVec2::splat(512);
        assert!(composite_quads(&bridge).is_empty(), "no cell yet: nothing");
        bridge.cells.insert(
            handle,
            Cell {
                origin: UVec2::new(67, 2),
                size: UVec2::new(63, 63),
            },
        );
        // A cell the reaper's request drop orphaned: nothing to place it at.
        bridge.cells.insert(
            stray,
            Cell {
                origin: UVec2::new(2, 2),
                size: UVec2::new(63, 63),
            },
        );
        let quads = composite_quads(&bridge);
        assert_eq!(quads.len(), 1, "one cell with a request draws once");
        let q = &quads[0];
        assert_eq!(q.rect, req.rect);
        assert_eq!(q.z_key, req.z_key);
        assert_eq!(q.color, [1.0, 1.0, 1.0, 0.5], "the frame's own alpha");
        assert!(q.premultiplied);
        assert_eq!(q.clip, req.clip);
        assert!(q.texture.is_some());
        let [tl, _, br, _] = q.uv.corners;
        assert!((tl[0] - 67.0 / 512.0).abs() < 1e-6 && (tl[1] - 2.0 / 512.0).abs() < 1e-6);
        assert!((br[0] - 130.0 / 512.0).abs() < 1e-6 && (br[1] - 65.0 / 512.0).abs() < 1e-6);
    }

    #[test]
    fn the_park_verdict_is_exactly_what_the_composite_draws() {
        let mut arena = benilla_ui::widget::WidgetArena::new();
        let mut bridge = UiModelTiles {
            atlas: Some(Handle::default()),
            atlas_size: UVec2::splat(512),
            ..Default::default()
        };
        // Three panes on the paint list; the packer placed the first two and ran out of atlas.
        let panes: Vec<FrameHandle> = (0..3)
            .map(|_| arena.create(benilla_ui::widget::FrameKind::Frame, None, None))
            .collect();
        for (i, &h) in panes.iter().enumerate() {
            bridge.requests.insert(
                h,
                TileRequest {
                    path: r"Interface\Buttons\UI-AutoCastButton.mdx".into(),
                    size_px: UVec2::new(63, 63),
                    px_per_unit: 1.0,
                    pos_px_per_unit: 1.0,
                    star_px_per_unit: 1.0,
                    facing: 0.0,
                    position: Vec3::ZERO,
                    root_scale: 1.0,
                    root_pos: Vec3::ZERO,
                    camera: None,
                    light: ModelLight::default(),
                    fog: None,
                    icon: None,
                    rect: Rect::new(0.0, 0.0, 63.0, 63.0),
                    z_key: 1_000 + i as u64,
                    alpha: 1.0,
                    clip: None,
                },
            );
            if i < 2 {
                bridge.cells.insert(
                    h,
                    Cell {
                        origin: UVec2::new(2 + 65 * i as u32, 2),
                        size: UVec2::new(63, 63),
                    },
                );
            }
        }
        let drawn: std::collections::HashSet<u64> =
            composite_quads(&bridge).iter().map(|q| q.z_key).collect();
        assert_eq!(drawn.len(), 2, "the packer placed two, so two quads");
        for (i, &h) in panes.iter().enumerate() {
            assert_eq!(
                draws_this_frame(&bridge, &h),
                drawn.contains(&(1_000 + i as u64)),
                "pane {i}: the park verdict and the composite must agree"
            );
        }
        // A repack moves texel windows without changing who has a cell.
        bridge.atlas_size = UVec2::splat(1024);
        let regrown: std::collections::HashSet<u64> =
            composite_quads(&bridge).iter().map(|q| q.z_key).collect();
        assert_eq!(
            regrown, drawn,
            "a repack changes texel windows, not the drawn set"
        );
    }

    /// `UI-Cooldown-Indicator.m2` as `benilla-extract m2seq` reads it: two clamped 1000 ms
    /// sequences, ids 0 (the sweep) and 1 (the flash).
    const COOLDOWN_FILE: &str = r"Interface\Cooldown\UI-Cooldown-Indicator.mdx";
    fn cooldown_facts() -> ModelFileFacts {
        ModelFileFacts {
            sequences: vec![
                SequenceFacts {
                    anim_id: 0,
                    duration_ms: 1000,
                    looping: false,
                },
                SequenceFacts {
                    anim_id: 1,
                    duration_ms: 1000,
                    looping: false,
                },
            ],
            bbox: ([0.0; 3], [0.0; 3]),
            cameras: 0,
        }
    }

    /// A fresh VM with a shown cooldown pane, the `CooldownFrameTemplate` (`Cooldown.xml`) every
    /// action button inherits.
    fn vm_with_a_cooldown_pane() -> UiScript {
        let mut s = UiScript::new().expect("VM");
        s.set_screen_size(1024.0, 768.0);
        s.run(&format!(
            r#"cd = CreateFrame("Model", "CD", UIParent) cd:SetWidth(36) cd:SetHeight(36)
               cd:SetPoint("CENTER", UIParent, "CENTER", 0, 0) cd:SetModel("{}")"#,
            COOLDOWN_FILE.replace('\\', "\\\\")
        ))
        .expect("build the pane");
        s.resolve();
        s
    }

    /// `model_facts_wanted` drains and only `SetModel` re-pushes, so a want skipped because the
    /// asset is resident would leave every `<Model>` pane dark for the second VM's whole life.
    #[test]
    fn a_rebuilt_vm_is_told_about_a_file_the_host_already_loaded() {
        let key = benilla_ui::widget::model_key(COOLDOWN_FILE);
        let mut state = TileState::default();

        // Session 1: nothing is resident, so the host is asked to load the file.
        let mut first = vm_with_a_cooldown_pane();
        assert!(
            first.visible_model_panes().is_empty(),
            "no facts yet ⇒ the pane is not on the paint list (the reference's draw gate)"
        );
        assert_eq!(state.answer_facts(&mut first), vec![key.clone()]);
        // …the asset lands: the facts are handed over and cached beside the handle.
        state
            .loaded
            .insert(key.clone(), (Handle::default(), cooldown_facts()));
        first.set_model_facts(COOLDOWN_FILE, cooldown_facts());
        assert_eq!(first.visible_model_panes().len(), 1);

        // Session 2, the logout/login (or `/reload`) rebuild: a fresh VM, the same file.
        let mut second = vm_with_a_cooldown_pane();
        assert!(
            second.visible_model_panes().is_empty(),
            "a fresh VM starts knowing nothing about any file"
        );
        assert!(
            state.answer_facts(&mut second).is_empty(),
            "the file is resident here — there is nothing left to load"
        );
        assert!(
            second.has_model_facts(COOLDOWN_FILE),
            "the want was drained; if it is not answered NOW it is never answered again"
        );
        assert_eq!(
            second.visible_model_panes().len(),
            1,
            "the second session's cooldown pane must paint exactly like the first's"
        );
    }

    #[test]
    fn a_new_vm_inherits_nothing_keyed_by_a_frame_handle() {
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let mut table = MatAnimTable::default();
        let mut bridge = UiModelTiles::default();
        let mut state = TileState::default();

        let mut arena = benilla_ui::widget::WidgetArena::new();
        let handle = arena.create(benilla_ui::widget::FrameKind::Frame, None, None);
        let clip_slot = table.alloc().expect("a fresh table has rows");
        state.tiles.insert(
            handle,
            Tile {
                root,
                key: benilla_ui::widget::model_key(COOLDOWN_FILE),
                icon: None,
                m2: Handle::default(),
                built: true,
                light_slot: 0,
                cam_slot: None,
                clips: HashMap::new(),
                armed: None,
                clip_slot: Some(clip_slot),
                alpha_parts: Vec::new(),
                uv_parts: Vec::new(),
                emitters: Vec::new(),
                last_seen: 1,
                parked: false,
            },
        );
        bridge.requests.insert(
            handle,
            TileRequest {
                path: COOLDOWN_FILE.into(),
                size_px: UVec2::new(36, 36),
                px_per_unit: 1.0,
                pos_px_per_unit: 1.0,
                star_px_per_unit: 1.0,
                facing: 0.0,
                position: Vec3::ZERO,
                root_scale: 1.0,
                root_pos: Vec3::ZERO,
                camera: None,
                light: ModelLight::default(),
                fog: None,
                icon: None,
                rect: Rect::new(0.0, 0.0, 36.0, 36.0),
                z_key: 1,
                alpha: 1.0,
                clip: None,
            },
        );
        bridge.cells.insert(
            handle,
            Cell {
                origin: UVec2::splat(2),
                size: UVec2::splat(36),
            },
        );

        let one = UiScript::new().expect("VM");
        let two = UiScript::new().expect("VM");
        assert_ne!(one.session(), two.session(), "each VM has its own identity");

        let mut apply = |state: &mut TileState, bridge: &mut UiModelTiles, s: Option<&UiScript>| {
            let mut queue = bevy::ecs::world::CommandQueue::default();
            {
                let mut commands = Commands::new(&mut queue, &world);
                state.adopt_vm(s, bridge, &mut commands, &mut table);
            }
            queue.apply(&mut world);
        };

        // Adopting the VM these tiles belong to changes nothing…
        state.session = one.session();
        apply(&mut state, &mut bridge, Some(&one));
        assert_eq!(state.tiles.len(), 1);
        assert_eq!(bridge.requests.len(), 1);

        // …and the rebuild drops the lot.
        apply(&mut state, &mut bridge, Some(&two));
        assert!(
            state.tiles.is_empty(),
            "a dead VM's tiles must not be reused"
        );
        assert!(bridge.requests.is_empty() && bridge.cells.is_empty());
        assert!(
            world.get_entity(root).is_err(),
            "the tile's tree goes with it — a live root would keep drawing into the atlas"
        );
        assert_eq!(
            table.alloc(),
            Some(clip_slot),
            "the tile's mat-anim rows go back to the table"
        );
    }

    /// A perspective-leg `TileRequest` with only the terms the leg reads.
    fn persp_req(size: UVec2, root_scale: f32, root_pos: Vec3, facing: f32) -> TileRequest {
        TileRequest {
            path: String::new(),
            size_px: size,
            px_per_unit: 1.0,
            pos_px_per_unit: 1.0,
            star_px_per_unit: 1.0,
            facing,
            position: Vec3::ZERO,
            root_scale,
            root_pos,
            camera: Some(0),
            light: ModelLight::default(),
            fog: None,
            icon: None,
            rect: Rect::new(0.0, 0.0, 1.0, 1.0),
            z_key: 0,
            alpha: 1.0,
            clip: None,
        }
    }

    /// `HumanMale`'s camera 1 as `benilla-extract m2cam` reads it, matching the reference to the
    /// digit.
    fn human_male_cam1() -> benilla_assets::PortraitCamera {
        let wow = benilla_assets::coords::wow_to_bevy;
        benilla_assets::PortraitCamera {
            eye: wow([3.6585, 0.0338, 0.9227]),
            target: wow([-0.3644, 0.0291, 0.9873]),
            roll: 0.0,
            fov: 0.97991,
            near: 0.222_222_22,
            far: 27.777_779,
        }
    }

    /// The client's `0x5c3cc0`, a diagonal fov: `t = tan(fovy / (2·√(aspect²+1)))`,
    /// `m11 = 1/t`, `m00 = m11/aspect`. The numbers are the reference's worked checks:
    /// `θ = 0.287938 · fov` at 318×224, `0.346523 · fov` at 233×224, and `t = 0.1449700` for the
    /// fallback camera's `fov = 0.5`.
    #[test]
    fn the_perspective_projection_is_the_clients_diagonal_fov_matrix() {
        use bevy::camera::CameraProjection;

        // The fallback camera's worked line: `fov = 0.5` at the pet pane's 318×224.
        let cam = benilla_assets::PortraitCamera {
            eye: Vec3::new(0.0, 0.0, 5.0),
            target: Vec3::ZERO,
            roll: 0.0,
            fov: 0.5,
            near: 1.0 / 36.0,
            far: 5000.0,
        };
        let aspect: f32 = 318.0 / 224.0;
        assert!((aspect - 1.419_642_9).abs() < 1e-6);
        let m = pane_projection(&cam, aspect).get_clip_from_view();
        let t = 1.0 / m.y_axis.y;
        assert!((t - 0.144_97).abs() < 1e-5, "t = {t}");
        assert!(
            (m.x_axis.x - m.y_axis.y / aspect).abs() < 1e-6,
            "m00 = m11/aspect: {} vs {}",
            m.x_axis.x,
            m.y_axis.y / aspect
        );
        // The two worked half-angles, as fractions of the record fov.
        for (w, h, want) in [(318.0, 224.0, 0.287_938), (233.0, 224.0, 0.346_523_f32)] {
            let a: f32 = w / h;
            let theta = (0.5_f32 * cam.fov / (a * a + 1.0).sqrt()) / cam.fov;
            assert!((theta - want).abs() < 1e-5, "{w}x{h}: {theta} vs {want}");
        }
    }

    /// The eye and target ride the root the geometry is drawn through (`0x718960`), so a model
    /// point's clip-space `x/w` and `y/w` do not move with scale or offset.
    #[test]
    fn scale_and_position_cancel_on_the_perspective_leg() {
        use bevy::camera::CameraProjection;

        let cam = human_male_cam1();
        let aspect: f32 = 318.0 / 224.0;
        // Points spread over a character-sized body, in model space.
        let probes = [
            Vec3::ZERO,
            benilla_assets::coords::wow_to_bevy([0.0, 0.0, 1.8]),
            benilla_assets::coords::wow_to_bevy([0.3, -0.4, 1.0]),
            benilla_assets::coords::wow_to_bevy([-0.2, 0.5, 0.2]),
        ];
        let ndc = |req: &TileRequest| -> Vec<Vec2> {
            let leg = perspective_rig(&cam, req, aspect);
            let clip = leg.projection.get_clip_from_view() * leg.camera.to_matrix().inverse();
            probes
                .iter()
                .map(|p| {
                    let c = clip * leg.root.to_matrix() * p.extend(1.0);
                    Vec2::new(c.x / c.w, c.y / c.w)
                })
                .collect()
        };
        let base = ndc(&persp_req(UVec2::new(318, 224), 1.0, Vec3::ZERO, 0.0));
        for (label, req) in [
            (
                "SetModelScale(3)",
                persp_req(UVec2::new(318, 224), 3.0, Vec3::ZERO, 0.0),
            ),
            (
                "SetModelScale(0.25)",
                persp_req(UVec2::new(318, 224), 0.25, Vec3::ZERO, 0.0),
            ),
            (
                "SetPosition(0.4, -0.3, 0.9)",
                persp_req(UVec2::new(318, 224), 1.0, Vec3::new(0.4, -0.3, 0.9), 0.0),
            ),
            (
                "both at once",
                persp_req(UVec2::new(318, 224), 2.5, Vec3::new(-1.0, 2.0, 0.5), 0.0),
            ),
        ] {
            for (a, b) in base.iter().zip(ndc(&req)) {
                assert!(
                    (a.x - b.x).abs() < 2e-4 && (a.y - b.y).abs() < 2e-4,
                    "{label}: {a:?} vs {b:?} — the camera must ride the same root the model does"
                );
            }
        }

        // On the orthographic leg scale does not cancel: `px_per_unit` is the model's size.
        assert_ne!(
            persp_req(UVec2::new(318, 224), 1.0, Vec3::ZERO, 0.0).px_per_unit,
            0.0
        );
    }

    /// At roll 0 the up `0x7ac640` builds is model-space `+Z`, the axis `SetFacing` turns about, so
    /// eye, target, geometry and up turn together; on `<PlayerModel>`'s frozen camera (`0x7acf10`)
    /// a facing does show.
    #[test]
    fn facing_cancels_on_the_perspective_leg_too() {
        use bevy::camera::CameraProjection;

        let cam = human_male_cam1();
        let aspect: f32 = 318.0 / 224.0;
        let probes = [
            benilla_assets::coords::wow_to_bevy([0.0, 0.0, 1.8]),
            benilla_assets::coords::wow_to_bevy([0.3, -0.4, 1.0]),
            benilla_assets::coords::wow_to_bevy([-0.2, 0.5, 0.2]),
        ];
        let ndc = |facing: f32| -> Vec<Vec2> {
            let req = persp_req(UVec2::new(318, 224), 1.0, Vec3::ZERO, facing);
            let leg = perspective_rig(&cam, &req, aspect);
            let clip = leg.projection.get_clip_from_view() * leg.camera.to_matrix().inverse();
            probes
                .iter()
                .map(|p| {
                    let c = clip * leg.root.to_matrix() * p.extend(1.0);
                    Vec2::new(c.x / c.w, c.y / c.w)
                })
                .collect()
        };
        let base = ndc(0.0);
        for facing in [0.61, 1.0, std::f32::consts::PI, -2.4] {
            for (a, b) in base.iter().zip(ndc(facing)) {
                assert!(
                    (a.x - b.x).abs() < 2e-4 && (a.y - b.y).abs() < 2e-4,
                    "facing {facing}: {a:?} vs {b:?} — the camera turns with the model"
                );
            }
        }
        // On the orthographic leg a facing turns the model in the screen plane.
        let turned = wow_to_screen()
            * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
            * benilla_assets::coords::wow_to_bevy([1.0, 0.0, 0.0]);
        assert!((turned - Vec3::Y).length() < 1e-5, "{turned}");
    }

    /// `HumanMale`'s camera 1: the eye 4.02 model units in front of the target on the model's own
    /// `+X`, at chest height, looking at it with WoW `+Z` up.
    #[test]
    fn the_perspective_camera_is_the_records_lookat() {
        let cam = human_male_cam1();
        let leg = perspective_rig(
            &cam,
            &persp_req(UVec2::new(318, 224), 1.0, Vec3::ZERO, 0.0),
            318.0 / 224.0,
        );
        assert!(
            (leg.camera.translation - cam.eye).length() < 1e-5,
            "the eye"
        );
        let fwd = leg.camera.forward().as_vec3();
        let want = (cam.target - cam.eye).normalize();
        assert!((fwd - want).length() < 1e-5, "{fwd:?} vs {want:?}");
        // Each shipped model's record authors its own eye distance: that is what normalizes panes.
        let d = (cam.target - cam.eye).length();
        assert!((d - 4.0234).abs() < 1e-3, "authored eye distance {d}");
        // Up is the model's own up, not the camera's roll-free default in some other frame.
        assert!(leg.camera.up().as_vec3().dot(Vec3::Y) > 0.9);
    }

    #[test]
    fn the_axis_fix_is_the_ortho_legs_frame() {
        let q = wow_to_screen();
        let wow = |v: [f32; 3]| q * benilla_assets::coords::wow_to_bevy(v);
        assert!((wow([1.0, 0.0, 0.0]) - Vec3::X).length() < 1e-6);
        assert!((wow([0.0, 1.0, 0.0]) - Vec3::Y).length() < 1e-6);
        assert!((wow([0.0, 0.0, 1.0]) - Vec3::Z).length() < 1e-6);
        // A facing of +90° about WoW +Z turns +X into +Y on screen (CCW).
        let turned = q
            * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
            * benilla_assets::coords::wow_to_bevy([1.0, 0.0, 0.0]);
        assert!((turned - Vec3::Y).length() < 1e-5, "{turned}");
    }
}
