//! **UI model tiles** (decision 2008) — the renderer for a `<Model>` widget's M2: the cooldown
//! sweep, the autocast shine, the minimap and world-map pings, the item-push card, the map's
//! arrow, and whatever an addon parks in a `CreateFrame("Model")`.
//!
//! ## The law
//!
//! A `<Model>` **whose file supplies a camera** draws its scene through it: the widget holds a raw
//! camera index (ctor 0, `SetCamera(n)`), and the record's `lookAt(eye, target, up)` plus the
//! client's diagonal-FOV projection at the pane's own width/height frame the model (`0x76cec0`,
//! `0x7ac640`). The authored eye and target are carried through the model's root transform, so
//! `SetModelScale` and `SetPosition` cancel for framing on that leg and bite only on the record's
//! unscaled near/far. An index past the file's camera count installs a NULL camera, which is the
//! other leg:
//!
//! A `<Model>` with no M2 camera picked draws its scene **orthographically over the frame's
//! rect**: origin at the rect's bottom-left, `+X` right, `+Y` up, `Z` depth only, with the root
//! matrix `T(pos · layoutScale) · R(facing, +Z) · S(G48 · 5/3 · modelScale · layoutScale)` — so one
//! model unit is `1280 · modelScale · layoutScale` FrameXML units, aspect-independent (`0x7ad7f0`,
//! `0x76d1a0`). Every batch draws once, LEQUAL over a depth buffer cleared for the widget's own
//! rect, straight into the back buffer (`0x76d240`, `0x70b360`); every in-game UI M2 is UNLIT on
//! every material (measured over the shipped files); the animator is the world's, on the widget's
//! private clock (`0x714260`, `0x76d7f0`). A particle's half-extent is the one quantity outside the
//! unit law — added in eye space, it maps at `768 · √(a²+1)` FrameXML units per model unit and
//! carries neither scale (`0x7b2a50`).
//!
//! ## The shape here: tiles in one atlas, composited at the callback rank
//!
//! The reference draws into the back buffer between two 2-D batches; this engine's UI is one
//! quad pass, so a scene becomes a **tile**: every visible pane holding a file renders into its
//! own cell of one shared render-target atlas, at the pane's device-pixel size, through ONE
//! orthographic camera whose view is the atlas plane — each tile's model root is placed at its
//! cell, scaled to pixels per model unit, and the camera never moves. [`compose_tiles`] then
//! draws every cell as a premultiplied quad over its pane's rect at `ZKey::callback(Artwork)`
//! (1995's rank — after every texture and font string of the pane's layer), which is the same
//! picture the reference's callback drain produces: the cell is cleared to transparent like the
//! reference clears its depth, the 2-D layers under it stay under it, and the ones over it stay
//! over it. Cells never overlap, so one depth buffer serves every tile.
//!
//! **The composite is this renderer's per-frame output, never the extract's** (decision 2023).
//! The extract's `ModelPane` arm publishes the request — the pane's rect, paint key, alpha and
//! clip beside the unit ladder — and pushes no quad; the quad is appended in the
//! [`UiQuadAppend`] lane (the minimap fill's lane) from THIS frame's cells. The first shape had
//! the arm draw the cell it found in the bridge, which is last frame's at best and, because the
//! conversion is memoized on the engine's list, usually never: a cooldown armed on a quiet
//! interface extracted once (no cell yet), the cell arrived a frame later, and nothing ever
//! re-ran the conversion — the sweep drew only while the interface happened to be churning (the
//! stance bar at UI load), and never on an action press.
//!
//! The pipeline is the booths' (`crate::portrait`): the same HDR view shape, the same
//! `FfxGlow::UI_PANE` decode, the same material twin with only the light storage swapped, the
//! same collapsed rig lane and palette mirror, the same effect lane. What is new is the
//! orthographic preset, the atlas packing, and the clock: a tile samples the file at the play
//! head the ENGINE holds (decision 2007 — `UiScript::visible_model_panes`, read once per frame),
//! so the `AnimationPlayer` is paused and seeked rather than advanced, and every per-sequence
//! material track is sampled off that same cursor.
//!
//! ## The two legs in this shape
//!
//! The **orthographic** leg is the atlas's one camera: every pane on it shares that camera and its
//! layer, and its cell is where its root is placed. The **perspective** leg cannot share a camera
//! with anything — the projection and the view are the pane's own — so a perspective pane takes a
//! slot of a small pool ([`UI_MODEL_CAM_LAYERS`]) and gets a camera whose **viewport is its cell of
//! the same atlas** and whose layer is its own. That is the reference's own structure: one target,
//! one viewport per widget, depth cleared per widget. The orthographic camera runs first and is
//! the one that CLEARS the atlas; the perspective cameras load into it and clear only depth. And
//! because the perspective root sits in model space rather than at a cell, each of those cameras
//! needs a layer of its own or it would draw every other perspective pane over its own cell.
//!
//! ## What a tile's light is
//!
//! `<Model>`'s embedded light is DISABLED (ctor `0x76c8e0`), and a LIT batch under no light renders
//! black — which never shows on the shipped UI M2s because all of them are unlit, and which is the
//! faithful answer for an addon's lit one. So the DEFAULT tile light buffer is a black light (no
//! ambient, no diffuse, fog off) and unlit batches bypass it.
//!
//! A pane that arms one — `SetLight(1, …)`, or fog through `SetFogColor` — takes a slot of the
//! light pool instead ([`TileRig`]): its own buffer, its own material twins, its scene folded the
//! way the reference's collector folds it (a directional light into probe slot 0's SH, a point
//! light into the point table, the fog into rows 4/5 with the batch's authored `UNFOGGED` bit left
//! to do its own work). Nothing in the shipped interface arms either, so the pool normally holds
//! exactly the one buffer the tiles have always had.
//!
//! ## Texture transforms (decision 2019)
//!
//! A batch whose texture transform animates gets a material of its own per tile — a clone of
//! the twin with two mat-anim rows: the translation delta (the world's lane, `anim_slots.x`)
//! and the **affine** row (`anim_slots.z`: rotation and scale as deltas from the identity), both
//! sampled here off the pane's play head at the sequence's file slot, never off the world clock.
//! The shader composes them as the reference does — `uv' = R((uv + t − p) ⊙ s) + p` — which is
//! how the cooldown indicator's four quadrant quads turn their mask into the clockwise sweep.
//!
//! ## What a pane that stops drawing costs: nothing (decision 2046)
//!
//! A pane that leaves the engine's paint list keeps its tile for [`TILE_LINGER_FRAMES`] so a
//! cooldown that re-arms every few seconds, a ping, or a bag that reopens keeps its tree — but
//! the tree has to cost NOTHING while it waits, and hiding the root was never enough. Two of a
//! tile's three per-frame costs do not travel down the scene graph at all: an emitter entity is
//! a world root the particle lane walks directly, and the global-sequence bone channels are
//! written by a driver that reads no `Visibility`. So a hidden tile is **parked**, explicitly,
//! on the frame it stops drawing — [`AnimParked`] on the root (which holds the pose evaluation,
//! the compose, the palette write and the global-sequence writes) and
//! `ParticleEmitter::set_frozen` on every emitter (pool + age held, no quads).
//!
//! The emitter half is not a nicety. The particle lane's own freeze asks a CAMERA whether its
//! scene is drawn, which is exact for a booth (one camera, one scene) and cannot be asked here:
//! every orthographic pane on the sheet shares one camera, and that camera stays active whenever
//! ANY cell is packed, because it is the camera that clears the atlas. Before the park, a hidden
//! autocast-shine pane's four spline emitters kept integrating and kept pushing quads for the
//! whole linger, at the root's last-written cell — which after a repack belongs to another pane.
//!
//! ## What is deliberately NOT here
//!
//! - The **character panes** (`PlayerModel`/`DressUpModel`/`TabardModel`) keep their booths
//!   (`crate::portrait`): the reference frames those through raw camera 1 **frozen** at load, or
//!   a fixed fallback camera, which is a different selection and a different lifecycle from this
//!   widget's live one.
//! - A pane's fog does not reach its **particles**: the effect lane carries its own fog enum and
//!   reads its span from a render-world params uniform rather than from the light buffer, so a
//!   fogged pane's cloud draws unfogged. No shipped UI M2 with an emitter is fogged (the shine
//!   and the pings are UNFOGGED or unlit anyway), and the honest fix is on the effect lane.

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

/// `WOW_TILE_TRACE=1` — the tile probe: one `tile-trace:` line per pane per frame from the
/// renderer (the request, the cell, the play head, the sampled alphas and the rows written) and
/// one from the extract's composite arm (the quad's rect, rank and alpha, or "no cell yet").
/// A pane that is on the engine's paint list but draws nothing names the gate it stopped at.
/// The `test_ui` cooldown tests prove the engine scrubs; this is the instrument for the half
/// they cannot reach — whether the tile exists, where it is, and what it sampled. Read once.
pub(crate) fn trace_on() -> bool {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("WOW_TILE_TRACE").as_deref() == Ok("1"));
    *ON
}

/// One pane's request for a tile this frame — what the extract knows about the widget: its
/// size on the device, the unit ladder `0x76d1a0` derives from it, and the Lua-set scene.
/// Published by the extract's `ModelPane` arm (keyed by the pane's frame handle, overwritten on
/// every conversion), read by [`sync_tiles`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TileRequest {
    /// The `SetModel` path, as written.
    pub path: String,
    /// The pane's rect on the device, whole pixels — the tile's cell size.
    pub size_px: UVec2,
    /// Device pixels per **model unit** for the geometry: `1280 · modelScale · layoutScale`
    /// FrameXML units per unit, times the seam scale, times the DPI.
    pub px_per_unit: f32,
    /// Device pixels per **layout unit** — `SetPosition`'s space (`T(pos · layoutScale)`):
    /// `768 · √(a²+1) · layoutScale` FrameXML units per unit, times seam and DPI.
    pub pos_px_per_unit: f32,
    /// Device pixels per model unit for a **particle's half-extent** — eye space, no scale:
    /// `768 · √(a²+1)` FrameXML units per unit, times seam and DPI.
    pub star_px_per_unit: f32,
    /// `SetFacing`, radians about the screen normal (CCW positive, the reference's `+Z`).
    pub facing: f32,
    /// `SetPosition`, layout units.
    pub position: Vec3,
    /// The **perspective** leg's root scale (decision 2027): `s = G48 · (5/3) · modelScale ·
    /// layoutScale`, a pure number — that leg has no pixels-per-unit, because its projection is
    /// the file's own camera and its viewport is the cell. Both this and [`Self::root_pos`]
    /// cancel for framing (the camera is carried through the same matrix) and bite only on the
    /// record's unscaled near/far.
    pub root_scale: f32,
    /// The perspective leg's root translation: `SetPosition · layoutScale`, model units.
    pub root_pos: Vec3,
    /// **Which leg.** The installed camera as a raw index into the file's table, or `None` for
    /// the NULL camera — the orthographic leg, which is every shipped in-game pane. Resolved by
    /// the engine against the file's camera count, exactly where `0x76cec0` resolves it.
    pub camera: Option<u32>,
    /// The pane's embedded `CGLight` — disabled on a fresh `<Model>`, which is what makes a LIT
    /// batch draw black.
    pub light: ModelLight,
    /// The pane's fog, only when armed.
    pub fog: Option<ModelFog>,
    /// `ReplaceIconTexture`'s path — the type-14 batches' texture.
    pub icon: Option<String>,
    /// The pane's rect on the window — y-down logical px, the quad pass's space — where the
    /// cell composites.
    pub rect: Rect,
    /// The pane's paint key: `ZKey::callback(Artwork)` (1995), the composite's rank.
    pub z_key: u64,
    /// The frame's OWN alpha (`0x76d120`): the composite draws at it.
    pub alpha: f32,
    /// The enclosing ScrollFrame clip, if any (decision 0112), in the quad pass's space.
    pub clip: Option<Rect>,
}

/// Where a tile sits in the atlas — texel space, `y` down — for the composite quad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub origin: UVec2,
    pub size: UVec2,
}

/// The extract ↔ renderer bridge (the `BoothPanes` pattern): requests in, cells and the atlas
/// out. Lives on `crate::portrait::BoothBridge` so the extract reaches it through the seam it
/// already holds.
#[derive(Resource, Default)]
pub(crate) struct UiModelTiles {
    /// The last request per pane. A pane that stops being extracted keeps its stale entry
    /// (the memoized conversion cannot tell the renderer it vanished); which panes DRAW is the
    /// engine's paint list, never this map.
    pub requests: HashMap<FrameHandle, TileRequest>,
    /// This frame's cell per tile that has something to draw.
    pub cells: HashMap<FrameHandle, Cell>,
    /// The atlas image and its size — `None` until the first tile.
    pub atlas: Option<Handle<Image>>,
    pub atlas_size: UVec2,
    /// The window's DPI as the last extract saw it (device px per logical px) — the extract
    /// writes it, the arm reads it to size cells.
    pub dpi: f32,
}

/// A pane's whole light input: its embedded `CGLight` and its armed fog. Two panes with the same
/// scene share one light buffer and one material-twin cache; the DEFAULT scene — a disabled light
/// and no fog — is every shipped in-game pane, and it is slot 0.
#[derive(Clone, Copy, PartialEq)]
struct TileScene {
    light: ModelLight,
    fog: Option<ModelFog>,
}

impl TileScene {
    /// The `<Model>` ctor's scene: white, **disabled**, unfogged. What every pane the shipped
    /// interface draws holds, and what makes a LIT batch render black.
    fn default_scene() -> Self {
        Self {
            light: ModelLight::default(),
            fog: None,
        }
    }

    /// The light buffer this scene wants — the reference's collector, finalized.
    ///
    /// The collector is zeroed every frame and gathers only what the fill callback `0x76d680`
    /// stages: the fog when it is armed, and the light when it is **enabled**.
    /// With the light off, the finalize writes zero ambient and zero diffuse and a LIT batch draws
    /// black — the reference's answer, not a gap. With it on, the type decides which arm:
    /// directional folds ambient + one diffuse lobe through the SH accumulators
    /// (`0x71bc70`/`0x71bce0`), point drops into the ≤4-nearest heap and contributes **no**
    /// ambient (`0x71bf90`).
    ///
    /// The lit lane here is the rig one, so the ambient and the lobe go into **probe slot 0** —
    /// the slot every tile part's `MeshTag` names — exactly as the glue booth's own scene blob
    /// does; rows 0-2 are dead on that lane.
    fn blob(&self) -> LightBlob {
        let l = self.light;
        let (ambient, lobes, point) = match (l.enabled, l.omni) {
            (false, _) => ([0.0; 3], Vec::new(), None),
            // Type 1 (point/omni): the position is model space, the falloff is the shared
            // `1/(0.7d + 0.03d²)` the CGLight ctor's attenuation names and the point lane
            // already implements — so no range gate, the falloff does the bounding.
            (true, true) => (
                [0.0; 3],
                Vec::new(),
                Some((benilla_assets::coords::wow_to_bevy(l.vector), l.diffuse)),
            ),
            // Type 0 (directional): `CGLight+0x24` is the direction the light **PROPAGATES**,
            // and an SH lobe is centred on the TOWARD-light vector — so it goes in negated.
            // That sign is `0x71bce0`'s own: the moments at `collector+0x18…+0x50` take `d` as it
            // is, and then three `fchs` (`71be7c`/`71be81`/`71be86`) negate it before the nine SH
            // basis terms are built from it. The two families take opposite signs off the one
            // field, which is exactly how two careful readers can disagree about "the direction"
            // without either misreading a byte; the lobe's is the negated one.
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

/// One light rig of the pool: the scene it was built for, its GPU buffer, the mirror key it
/// registered under, and the material twins bound to it.
struct TileLight {
    scene: TileScene,
    buffer: Buffer,
    variants: HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
}

/// The candidacy range packed with a pane's point light. The reference's gather has no range gate
/// at all (a sorted ≤4-nearest heap), so this is effectively unbounded and the `1/(0.7d + 0.03d²)`
/// falloff does the bounding — the same number and the same reason as the glue scene's.
const POINT_RANGE: f32 = 1.0e6;

/// The mirror keys the light pool registers under, one per slot. Static because both mirror maps
/// are keyed by `&'static str`, and monotone because a key is only ever ADDED: the mat-anim
/// upload gates on the mirror count, so a set that grew and shrank in one frame could skip an
/// upload and leave a fresh buffer's rows at the identity.
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

/// The renderer's own fixed state: the ortho tiles' layer, and the light pool.
///
/// **The pool is lazily grown and capped.** Slot 0 is the default scene and is built at startup;
/// a pane that arms `SetLight(1, …)` or fog takes the next free slot, and a scene past the cap
/// falls back to slot 0 (drawn as if it had armed nothing). Every slot costs a whole shared-light
/// buffer — several megabytes, most of it the palette and probe regions it must carry to be
/// bindable at all — so the pool is not a per-pane map: the shipped interface arms neither verb
/// on any pane, so in practice it holds exactly the one buffer the tiles have always had.
#[derive(Resource)]
struct TileRig {
    layer: RenderLayers,
    lights: Vec<TileLight>,
}

impl TileRig {
    /// The pool slot for `scene`, growing the pool if it is new and there is room; `0` (the
    /// default scene's) when there is not.
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

/// Marks the tile atlas's one **orthographic** camera — the leg every pane without a file camera
/// takes, and the camera that CLEARS the atlas (which is why it stays active whenever any cell is
/// packed, even with no ortho tile on it: the perspective cameras load, they never clear).
#[derive(Component)]
struct TileCamera;

/// One of the perspective pool's cameras — `slot` is its layer and its order offset, held for the
/// life of the app and aimed at whichever pane holds the slot this frame.
#[derive(Component)]
pub(crate) struct TilePerspectiveCamera {
    slot: usize,
}

/// Marks a tile's root entity (the model root, at its cell or in its own perspective layer).
#[derive(Component)]
pub(crate) struct TileRoot;

/// One batch of a built tile whose alpha the file animates: sampled here off the pane's play
/// head (a hosted `MatAnim` would read the paused player, which has no node for a sequence
/// that keys no bone — the cooldown's sweep is exactly that).
struct AlphaPart {
    entity: Entity,
    anim: Arc<benilla_formats::AlphaAnim>,
}

/// One batch whose texture transform animates: the tile's OWN clone of the batch's material
/// (two panes on one file must not share a row — two cooldowns at different fractions), with
/// the table rows it writes per frame off the pane's play head (decision 2019).
struct UvPart {
    /// Held so the clone outlives its parts' handles by exactly the tile's lifetime.
    #[allow(dead_code)]
    material: Handle<WowModelMaterial>,
    /// The translation row: its slot, and the built seed the delta is measured from
    /// (`sun_scale.zw`, the loop's sample at 0).
    trans: Option<(u16, [f32; 2])>,
    /// The affine row's slot — rotation and scale ([`affine_row`]).
    affine: Option<u16>,
    uv_anim: Option<Arc<UvAnim>>,
    uv_seq: Option<Arc<SeqLoops<[f32; 2]>>>,
    uv_rot: Option<Arc<SeqLoops<[f32; 4]>>>,
    uv_scale: Option<Arc<SeqLoops<[f32; 2]>>>,
}

impl UvPart {
    /// Write this frame's rows for the sequence at `(seq_slot, cursor_s)` on the pane's clock
    /// `gseq_s`: the translation delta (quantized like the world's lane), and the affine row
    /// from the raw quaternion and the scale.
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

/// A live tile: its entity tree and what it was built from.
struct Tile {
    root: Entity,
    /// The file key the tree was built for (a `SetModel` to another file rebuilds).
    key: String,
    /// The icon override the tree was built with (a change rebuilds the materials).
    icon: Option<String>,
    m2: Handle<M2Model>,
    /// The tree is spawned (parts, rig, emitters) — until then the root is bare.
    built: bool,
    /// The light-pool slot the tree's materials and emitters are bound to. A pane whose light or
    /// fog changes lands on a different slot, and the tree is rebuilt against it — a material
    /// twin points at exactly ONE light buffer.
    light_slot: usize,
    /// The perspective pool slot this tile holds, and therefore its render layer — `None` for an
    /// orthographic tile, which shares the atlas camera's one layer. Also a rebuild key: the
    /// layer is on every spawned entity.
    cam_slot: Option<usize>,
    /// The graph node per `AnimationData` id the file keys a bone for, and its file slot.
    clips: HashMap<u16, (AnimationNodeIndex, usize)>,
    /// Which id the player is currently arming (to re-arm only on change).
    armed: Option<u16>,
    /// The mat-anim row this tile's materials read their **cell clip** from
    /// (`anim_slots.w`, decision 2093): `[min.x, min.y, max.x, max.y]` in atlas texels, written
    /// every frame from the cell. `None` only when the table was full at build — the tile then
    /// draws unclipped, which is the pre-2093 picture rather than a missing widget.
    clip_slot: Option<u16>,
    alpha_parts: Vec<AlphaPart>,
    uv_parts: Vec<UvPart>,
    emitters: Vec<Entity>,
    /// The last frame this tile was on the engine's paint list.
    last_seen: u64,
    /// Parked: this tile drew no cell last frame, so its rig and its emitters are held (decision
    /// 2046; the module doc's "what a pane that stops drawing costs"). A fresh tile starts
    /// `false` and is parked by the same walk on its first non-drawing frame, so there is one
    /// code path and no spawn-time special case.
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

/// Frames a tile survives off the paint list before its tree is torn down — long enough that a
/// cooldown that re-arms every few seconds, or a ping, keeps its tree.
const TILE_LINGER_FRAMES: u64 = 600;

/// The atlas edge the first tile allocates, and the cap a grown atlas stops at.
const ATLAS_MIN: u32 = 512;
const ATLAS_MAX: u32 = 4096;

/// Gutter between cells (texels): a tile's bilinear edge never samples a neighbour.
const GUTTER: u32 = 2;

/// The tile camera's order — after every booth (`-100 …`), before the UI camera (`1`).
const TILE_CAMERA_ORDER: isize = -10;

/// The renderer's per-frame state that is not the bridge.
///
/// **Most of this is about a VM, and the VM does not live for the process** (decision 1290): it
/// is built at world entry and destroyed at the character screen, and `ReloadUI()` is both edges
/// back to back. [`TileState::session`] is what keeps that honest — see [`TileState::adopt_vm`]
/// and [`TileState::answer_facts`].
#[derive(Default)]
struct TileState {
    /// The VM these tiles and the bridge's handle-keyed maps belong to
    /// ([`UiScript::session`]); `0` = none (the "no VM" branch, and a freshly built state).
    session: u64,
    tiles: HashMap<FrameHandle, Tile>,
    /// Files the engine asked facts for, loading.
    pending_facts: HashMap<String, Handle<M2Model>>,
    /// Files whose facts have been derived: the asset handle (which keeps the file resident for
    /// the tiles) beside the facts themselves.
    ///
    /// **The facts are cached because they are owed to every VM, not to the process.** This map
    /// is a HOST fact — "the file is loaded here" — and it stood in for the per-VM fact "this
    /// VM's engine has been told about the file" until [`TileState::answer_facts`]; the module
    /// doc of `crate::ui_script::session` names that class and why it has no error path.
    loaded: HashMap<String, (Handle<M2Model>, ModelFileFacts)>,
    frame: u64,
}

impl TileState {
    /// Hand the engine the facts for every file it just asked about, and answer with the keys
    /// that still need loading (not resident here, and not already in flight).
    ///
    /// **Every ask gets an answer, whichever VM is asking.** `model_facts_wanted` DRAINS, and the
    /// only thing that re-pushes a want is `SetModel` — so a want that is dropped is dropped for
    /// the life of that VM. Skipping the answer because the file was already loaded *for an
    /// earlier VM* is therefore permanent: [`UiScript::visible_model_panes`] drops a pane whose
    /// file the engine knows nothing about, so from the second world entry on, every `<Model>`
    /// widget in the game — the cooldown pie and the GCD sweep, the autocast shine, the minimap
    /// and world-map pings, the item-push card, the map arrow — went dark until the process was
    /// restarted. That is decision 1290's class exactly: a host memory standing in for a per-VM
    /// one, failing silently, with the window simply empty.
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
            // Already in flight: the landing below answers whichever VM is asking by then.
            if self.pending_facts.contains_key(&key) {
                continue;
            }
            to_load.push(key);
        }
        to_load
    }

    /// Adopt the VM `script` names — **forgetting every handle-keyed memory when it is a
    /// different one than these tiles were built against**.
    ///
    /// A [`FrameHandle`] is a generational index into ONE VM's widget arena, and the VM is
    /// rebuilt at every logout, login and `ReloadUI()` (1290/1291). The next VM starts a fresh
    /// arena and reissues the SAME indices at the same generations, so a tile or a request that
    /// outlives its VM is not merely stale: it names a different frame. `None` is "no VM" — a
    /// session in its own right (session `0`), exactly as [`crate::ui_script::VmMemo`] treats the
    /// character screen.
    ///
    /// **When** this runs is load-bearing: [`forget_dead_vm_tiles`], ahead of the extract.
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
            // **Before the extract**, which is where a `<Model>` pane's tile request is
            // published (the UI pass's `paint_script`) — see [`forget_dead_vm_tiles`].
            .add_systems(
                Update,
                forget_dead_vm_tiles.in_set(crate::ui_script::UiFeed),
            )
            // After the extract published this frame's requests, and before the pose/palette
            // passes read the roots' transforms (they run in PostUpdate).
            .add_systems(
                Update,
                sync_tiles
                    .after(crate::ui_script::UiInput)
                    .after(forget_dead_vm_tiles),
            )
            // The composite: this frame's cells, appended in the lane the minimap fill uses —
            // after the cells are packed, before the mesh rebuild reads the lane.
            .add_systems(Update, compose_tiles.in_set(UiQuadAppend).after(sync_tiles))
            .add_systems(Update, reap_tile_variants)
            .add_systems(Update, dump_atlas.after(sync_tiles));
    }
}

/// Startup: the camera, the layer, the black light.
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
    // Slot 0 — the `<Model>` ctor's scene: light disabled, fog off (`0x76c8e0`). Built here
    // rather than lazily because every shipped pane wants it and nothing else ever does.
    //
    // Tile rigs skin from a light buffer's palette region (decision 0720's mirror law), and the
    // tiles' animated materials read their mat-anim rows from it too (decision 2023): a twin
    // binds ITS buffer, not the world's, so every buffer the pool creates joins both mirror
    // lists or the rows the tiles write every frame reach a buffer nothing in a tile samples.
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
            // The reference clears DEPTH for the widget's rect and leaves colour to the 2-D
            // pass; a tile composites over the 2-D pass instead, so its colour clears to
            // nothing — the premultiplied transparent the booth panes use (decision 1083).
            clear_color: ClearColorConfig::Custom(Color::NONE),
            is_active: false,
            ..default()
        },
        // Decode, no scene glow: a UI model draws in the UI strata after the WorldFrame's
        // FFX apply (decision 0638's law for the body panes, the same widget family).
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
    // The perspective pool: one camera per slot, each on its own layer, each parked inactive
    // until a pane takes the slot. They are built once here rather than spawned on demand — a
    // camera that appears mid-frame misses the render world's extract, and a pane that took its
    // slot this frame would draw nothing until the next.
    for slot in 0..UI_MODEL_CAM_LAYERS {
        commands.spawn((
            Name::new(format!("ui model pane camera {slot}")),
            booth_view_shape(),
            Camera {
                order: TILE_CAMERA_ORDER + 1 + slot as isize,
                // **Load, never clear.** These share the atlas with the orthographic camera,
                // which runs first (a lower order) and clears the whole target; a second clear
                // here would wipe the cells already drawn into it. Depth is a different matter
                // and clears per camera by default, which is the reference's own per-widget
                // depth clear (`76d5e1 GxClear(2)` inside the widget's viewport).
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

/// pipe_warm's **orthographic twin camera** (decision 2262) — the ortho leg's view key space, the
/// way [`crate::portrait::spawn_warm_booth`] is the custom-projection one (0958).
///
/// bevy_pbr folds the view's projection **class** into `MeshPipelineKey` (`bevy_pbr-0.18.1`
/// `render/mesh.rs:397` — `Perspective | Orthographic | Custom`, emitted as the
/// `VIEW_PROJECTION_*` shader def at `:2549`), so one material is a *different pipeline* per
/// class. 0958's census closed with "the whole 3-D view space is `(samples, projection class)`,
/// and both classes of both sample counts are now warm" — true on 2026-08-04, when the only
/// classes were the world camera's Perspective and the booths' custom `WowPortraitProjection`.
/// Decision 2013 added [`setup_tiles`]' orthographic camera a month later and nothing widened the
/// warm pass, so the first UI model tile of a session — a cooldown pie, a minimap ping, an
/// item-push card — compiled its batches live, uncovered, on the render thread.
///
/// This camera is that missing class in the tile camera's exact shape: [`booth_view_shape`]
/// (`Msaa::Off`, HDR, no tonemap) and `FfxGlow::UI_PANE`, spawned right beside the real one above
/// so the two cannot drift apart. The REAL tile camera is deliberately not borrowed for warming —
/// it is `is_active: false` until a pane packs a cell, and switching it on would draw the whole
/// menagerie into the live atlas. Only the projection's CLASS keys the pipeline, never its
/// numbers; they mirror the real camera's regardless, for the same anti-drift reason.
pub(crate) fn spawn_warm_tile_cam(
    commands: &mut Commands,
    images: &mut Assets<Image>,
) -> (Entity, RenderLayers) {
    let layer = RenderLayers::layer(crate::portrait::WARM_ORTHO_LAYER);
    // The atlas's own minimum size, the way `spawn_warm_booth` takes the real booths'. A render
    // target's SIZE reaches no pipeline key (the combine pair is keyed on format; mesh pipelines
    // specialise at queue time, before anything rasterises), so this could be tiny — and a 64²
    // arm was measured against this one: 4.59 s vs 4.61 s of warm drain, i.e. nothing. The pass's
    // extra cost is the wider cross it reveals, not the pixels this camera fills, so the size
    // stays the one that matches the camera being warmed.
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

/// The bevy-space → tile-camera-space rotation: WoW `+X` (bevy `−Z`) to the right, WoW `+Y`
/// (bevy `−X`) up, WoW `+Z` (bevy `+Y`) toward the viewer — the ortho leg's axes (`0x7ad7f0`).
/// A proper rotation (determinant +1), so winding survives.
fn wow_to_screen() -> Quat {
    Quat::from_mat3(&Mat3::from_cols(
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(-1.0, 0.0, 0.0),
    ))
}

/// The assets a tile build reads and writes, in one param (the 16-parameter ceiling).
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
    /// The batch materials — and, through it, the material store the twins are added to (a
    /// second `ResMut<Assets<WowModelMaterial>>` beside it would conflict at schedule time).
    mats: M2BatchMaterials<'w>,
    palettes: ResMut<'w, RigPalettes>,
    rig: ResMut<'w, TileRig>,
    /// The shared mat-anim table: the tiles' animated materials own rows in it (2019).
    table: ResMut<'w, MatAnimTable>,
    /// The light pool grows lazily, so the device, the queue and both mirror lists have to be
    /// reachable from the per-frame pass, not only from startup.
    device: Res<'w, RenderDevice>,
    queue: Res<'w, RenderQueue>,
    mirrors: ResMut<'w, RigPaletteMirrors>,
    anim_mirrors: ResMut<'w, MatAnimMirrors>,
}

/// **The VM edge** ([`TileState::adopt_vm`]) — its own system, and ordered **ahead of the
/// extract**, which is the whole reason it is not a first step inside [`sync_tiles`].
///
/// A logout, a login and a `ReloadUI()` all replace the VM in `PreUpdate`; the extract
/// (`ui_script`'s `paint_script`) then publishes the NEW tree's tile requests, and
/// `sync_tiles` reads them after that. Clearing the bridge from inside `sync_tiles` would
/// therefore throw away the new VM's very first publish — and the extract's conversion is
/// memoized on the engine's entry list, so an entry that does not change again is never converted
/// again and the request never comes back (decision 2023's defect, from the other side). Running
/// on the frame's way IN puts the clear before the publish instead of after it.
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
        // No VM: nothing paints. The tiles were retired on the edge
        // ([`forget_dead_vm_tiles`]); every frame after it must still composite nothing and
        // leave no live camera.
        bridge.cells.clear();
        set_camera_active(&mut cams, false);
        for (mut cam, _, _, _, _) in &mut pane_cams {
            cam.is_active = false;
        }
        return;
    };

    // ── 1. Facts: what the engine asked for — answered now when the file is already here
    //         ([`TileState::answer_facts`]), when it lands otherwise ─────────────────────
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
            continue; // not readable yet — stay in `pending_facts` rather than fall off the loop
        };
        let facts = facts_of(model);
        script.set_model_facts(&key, facts.clone());
        state.pending_facts.remove(&key);
        // Cached beside the handle so the next VM's want is answered without re-reading the
        // asset — and so the answer cannot depend on the asset still being resident.
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
            continue; // not extracted yet — next frame
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
    // Stable cell order: the engine's registry order (creation order), so a pane keeps its
    // cell across frames.
    let _ = &live;

    // ── 2a. Which light rig, and which perspective slot ─────────────────────────────────
    //
    // The light pool dedups by scene, so every default pane — which is every shipped one —
    // lands on slot 0 and shares one buffer and one twin cache, exactly as before 2027.
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
    // A perspective pane needs a camera of its own, and therefore a layer of its own. Slots are
    // sticky: a pane that already holds one keeps it, so a tile is not rebuilt every frame for
    // its layer, and the free ones go to whoever is new. A pane past the pool draws nothing —
    // the same degrade as a tile that does not fit the capped atlas.
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
            continue; // facts not landed ⇒ the engine would not have listed it; defensive
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
                    // **Named by its PANE, and at debug.** A tile is per-widget by construction
                    // (`state.tiles` is keyed by `FrameHandle`, and 2019 requires it: two panes on
                    // one file must not share their `MatAnimTable` rows, or two cooldowns at
                    // different fractions would fight). So five cooldowns up at once legitimately
                    // build five tiles — which, logged at info with only the PATH, arrived as five
                    // byte-identical lines that read like a caching bug. The path alone also
                    // recurs: `TILE_LINGER_FRAMES` is ~10 s, so any longer cooldown rebuilds and
                    // re-logs at combat rate.
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
                // A pane that wants the perspective leg and found no free camera holds no cell and
                // draws nothing — the same degrade as a tile that does not fit the capped atlas.
                // Falling through to the orthographic placement would draw it at that leg's pixel
                // ladder, which for a creature is a wall of fur.
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

        // The play head: the engine's cursor drives the paused player, every alpha track, every
        // material row — and, on the perspective leg, the camera's own authored path.
        let (armed, cursor_s, seq_slot) = match pane.play {
            Some(ph) => {
                let slot = tile.clips.get(&ph.anim_id).map(|&(_, s)| s);
                (Some(ph.anim_id), ph.cursor_ms as f32 / 1000.0, slot)
            }
            None => (None, 0.0, None),
        };

        // ── The leg ────────────────────────────────────────────────────────────────────
        //
        // PERSPECTIVE (`0x7ac640`): the file's own camera record frames the
        // pane, through a camera of this tile's own whose viewport is the tile's cell. The root
        // is `T(pos·layoutScale) · R(facing) · S(s)` in MODEL units — no pixel ladder, because
        // the projection is the record's and the viewport is the cell — and the authored
        // eye/target are carried through that same matrix, which is what `0x718960`'s publish
        // does and why `SetModelScale` and `SetPosition` cancel for framing here.
        //
        // ORTHOGRAPHIC: unchanged since 2013 — the model laid over the cell at
        // `1280 · modelScale · layoutScale` FrameXML units per model unit.
        let perspective = tile.cam_slot.zip(req.camera).and_then(|(slot, idx)| {
            let cam = assets.m2s.get(&tile.m2)?.cameras.get(idx as usize)?;
            Some((slot, cam.clone()))
        });
        let mut leg_trace = String::from("ortho");
        if let Some((slot, cam)) = perspective {
            // The camera's tracks read the file's ABSOLUTE timeline, like every other M2 track,
            // while the pane's play head is a cursor inside the armed band — so the band start
            // is what turns one into the other. Static on every camera a shipped pane can name.
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
                // The target FIRST, and the activation only if there is one: these cameras are
                // spawned with the default render target, which is the primary window — an
                // active one without the atlas installed would draw the model over the game.
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
            // The root: the cell's bottom-left in camera space, plus `SetPosition` in layout
            // units.
            let cell_bl = Vec2::new(
                cell.origin.x as f32,
                atlas_h - (cell.origin.y + cell.size.y) as f32,
            );
            let pos = Vec2::new(req.position.x, req.position.y) * req.pos_px_per_unit;
            let depth = req.position.z * req.pos_px_per_unit;
            // `T(pos) · R(facing about WoW +Z) · S(px per unit)`, in camera space: the facing
            // turns about bevy `+Y` (WoW's `+Z` after `wow_to_bevy`), then the axis fix, then
            // the scale.
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
        // The file slot the material tracks read: the armed sequence's, or the file's first
        // when the armed id keys no bone (its slot is still in the facts' order — the cooldown).
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
        // …and the BONE global-sequence channels read it too, not the world clock (decision
        // 2046). The animation kernel's Phase B cursor is `[[model+0x2c]+0xc] − [model+0x68]`
        // (`0x714260`) — the clock of the scene that OWNS the instance, minus the attach
        // snapshot — and a `<Model>` widget owns a private `CM2Scene` (`CSimpleModel+0x314`)
        // that only its own `OnUpdate` (`0x76d7f0`) advances. The visible case is the ping's
        // 4833 ms spinner: its phase belongs to the pane, which is why "ping N resumes where
        // ping N−1 stopped" needs no accumulator of ours (2013). The drive stamps its own anchor
        // on its first tick, which is the attach.
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
                // The `A ≤ 0` cull (`0x707b3a`): a batch the artist keyed
                // off in this sequence is skipped, not drawn at zero.
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
        // A particle's half-extent is added in EYE space, so its unit is the leg's: the
        // orthographic leg measures the cell in pixels and hands it `768·√(a²+1)` FrameXML units
        // per model unit (`0x7b2a50`, carrying neither scale), while the perspective leg's eye
        // space IS the root's, and its own projection does the conversion — the world's plain 1.0.
        let star = if tile.cam_slot.is_some() && req.camera.is_some() {
            1.0
        } else {
            req.star_px_per_unit
        };
        // …and the CELL the cloud may draw in (decision 2093). A tile's particles are quads in
        // the ATLAS's own space, so without this a cloud that reaches past its cell lands in the
        // cell the shelf packed beside it — which the composite hands to a different widget
        // (B379). `Cell::origin` is already in atlas texels, top-left origin, which is exactly
        // the framebuffer coordinate the fragment tests.
        let clip = Vec4::new(
            cell.origin.x as f32,
            cell.origin.y as f32,
            (cell.origin.x + cell.size.x) as f32,
            (cell.origin.y + cell.size.y) as f32,
        );
        // The MESH half of the same clip: the tile's own row, read by every one of its materials
        // through `anim_slots.w`.
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
    //
    // "Drawing" is exactly "has a cell this frame" — the same set the placement loop above wrote.
    // Parking is the module doc's contract: `AnimParked` holds the rig (the 0712 evaluator, the
    // compose, the palette write and the global-sequence bone writes), the freeze holds every
    // emitter's pool, age and quads, and the root is hidden so no batch draws. This walk replaces
    // the `hidden` vector the placement loop used to `retain` out of once per drawing tile — the
    // same verdict, without the quadratic.
    for (handle, tile) in state.tiles.iter_mut() {
        let park = !draws_this_frame(&bridge, handle);
        // The emitter freeze is COMPARED, not edge-triggered off `tile.parked`: a tile can be
        // built while it is already parked (its pane is on the paint list but its cell did not
        // fit the capped atlas), and an emitter is born thawed. The comparison is a `Deref`, so
        // a steady state touches no change tick.
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
            // The marker drops before `AnimationSystems` (this is `Update`, the lane is
            // `PostUpdate`), so the first thawed frame evaluates and composes before anything
            // reads the pose — 0739's wake law, the same as every world rig's.
            commands.entity(tile.root).remove::<AnimParked>();
        }
    }
    for (mut cam, _, _, _, marker) in &mut pane_cams {
        let want = aimed[marker.slot];
        if cam.is_active != want {
            cam.is_active = want;
        }
    }
    // The ORTHOGRAPHIC camera stays on whenever anything is packed, even with no ortho tile on
    // it: it is the one that clears the atlas, and every perspective camera loads.
    set_camera_active(
        &mut cams,
        !bridge.cells.is_empty() && bridge.atlas.is_some(),
    );
}

/// The perspective leg's rig for one pane — the pure half, so the reference's worked numbers can be
/// checked without a world.
struct PerspectiveRig {
    /// The model's root, `T(pos · layoutScale) · R(facing, +Z) · S(s)` in Bevy model space —
    /// `0x76d1a0`'s `model+0xbc`, minus the pixel ladder the orthographic leg needs.
    root: Transform,
    /// The camera, `lookAt(eye, target, up)`, with the eye as the view origin.
    camera: Transform,
    /// The record's projection at the pane's own width/height.
    projection: WowPortraitProjection,
}

/// Build it.
///
/// **The authored eye and target are carried through the root transform.** That is `0x718960`'s
/// publish — `eye_published = (position_base + posTrack) · M_root` — and it is the whole reason
/// `SetModelScale` and `SetPosition` cancel here: the camera moves with the model, so the framing
/// is invariant to both. They are still applied rather than skipped, because the record's near and
/// far are copied into the camera **unscaled** (`0x70ebd0`) while every eye-space depth scales
/// with `s`, so a LARGE `SetModelScale` drives the model through the **far** plane and a small one
/// through the near (the direction is the opposite of the obvious guess). `near`/`far` reach only
/// `m22`/`m32`: `near` cancels algebraically out of `m00`/`m11`, so the x/y screen scale is `fov`
/// and `aspect` alone.
///
/// The **up** vector does not ride the root: `0x7ac640` assembles it out of four `CCamera` fields
/// the publish never writes — `up = (sin(a₆)·sin(roll), −cos(a₆)·sin(roll), cos(roll))` in WoW
/// model space, and property `a₆` has **no writer image-wide**, so it holds its constructor `0`
/// for ever and the vector is `(0, −sin(roll), cos(roll))`. At `roll = 0` that is model-space `+Z`
/// exactly — which is the very axis `SetFacing` turns the model about, and *that* is why the facing
/// cancels here too: the eye, the target and the geometry all turn about an axis the up vector lies
/// on, so the image does not move. The reference's own one-frame publish lag — its paint `0x76d240`
/// reads the eye before it rebuilds the root, so the frame a facing CHANGES draws one step out of
/// phase — is a quirk of the ordering, not the mechanism, and is deliberately not reproduced.
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
    // `up = (0, −sin(roll), cos(roll))`, WoW model space — the camera's own, not a roll about the
    // view axis. They agree at `roll = 0`, which is every camera a `<Model>` pane can name.
    let (sin_roll, cos_roll) = record.roll.sin_cos();
    let up = benilla_assets::coords::wow_to_bevy([0.0, -sin_roll, cos_roll]);
    PerspectiveRig {
        root,
        camera: Transform::from_translation(eye).looking_at(target, up),
        projection: pane_projection(record, aspect),
    }
}

/// Reap the material twins whose world source material died — the same law and the same event as
/// the booths' (`portrait::light::reap_dead_variants`), which the tiles were missing: a twin is
/// its own asset pinned only by this cache, so without the reap every UI M2 material a session
/// ever showed would survive a map-scope teardown (which clears the world material cache, killing
/// exactly these keys) for the life of the process. A live tile keeps its twin through its own
/// `MeshMaterial3d`; only the dedup entry drops.
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

/// The render layer a tile's entities live on: the atlas camera's shared one for an orthographic
/// tile, and the perspective pool's own for a tile with a camera — one layer per camera, or every
/// perspective camera would draw every other pane's model over its cell.
fn tile_layer(rig: &TileRig, cam_slot: Option<usize>) -> RenderLayers {
    match cam_slot {
        Some(slot) => RenderLayers::layer(UI_MODEL_CAM_LAYER_BASE + slot),
        None => rig.layer.clone(),
    }
}

/// `WOW_TILE_DUMP=<path>:<secs>` — **shoot the tile atlas itself**, once, `secs` of app time in.
///
/// The composited frame is the wrong place to read a `<Model>` widget: over an action button or a
/// bag slot the pane sits on the button's own art, so every measurement of what the widget drew is
/// a measurement of the icon underneath it plus a sub-pixel alignment guess. The atlas cell is the
/// widget ALONE, on transparent, at exactly the size the pane asked for — and the trace's
/// `cell=(x,y WxH)` says where each pane's is. Together they answer "what did this widget
/// actually paint", which nothing else here can (B379: the cooldown's sweep read as a filmstrip,
/// one cell per phase).
///
/// The camera needs no waking, unlike the booths' twin (`portrait::test_bake::dump_booth_target`):
/// the orthographic tile camera is the one that CLEARS the atlas, so it is active whenever any
/// cell is packed — which is exactly when there is something to shoot. A dump that comes back
/// uniformly transparent means no pane was drawing, not a broken widget.
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
        return; // no atlas yet — wait for the first tile rather than shoot nothing
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

/// The composite: one premultiplied quad per cell packed THIS frame, over its pane's rect at the
/// pane's paint key and alpha, clipped as the pane is — appended to the UI pass's overlay lane
/// every frame (the lane is cleared at the top of [`UiQuadAppend`] and diffed by the rebuild, so
/// an unchanged set costs no re-batch). A pane with a request and no cell draws nothing; a cell
/// whose request vanished (the linger reaper) draws nothing.
pub(crate) fn compose_tiles(bridge: Res<UiModelTiles>, mut quads: ResMut<UiQuads>) {
    quads.overlays.extend(composite_quads(&bridge));
}

/// **Does this tile draw this frame?** — which is the park verdict inverted, and deliberately
/// ONE function beside [`composite_quads`] so the two cannot drift apart (decision 2046).
///
/// The answer is "the bridge holds a cell for it", and that is exact rather than approximate:
/// [`composite_quads`] draws precisely the `cells ∩ requests` pairs, and [`sync_tiles`] fills
/// `cells` in the same pass that reads it — stage 4 clears the map, stage 5 inserts a cell for
/// every drawing pane the packer placed, and that insert happens BEFORE any of the loop's later
/// `continue`s. So the two cases that look as though they could strand a visible pane cannot:
///
/// - an **atlas repack** (the 512→1024 growth) re-allocates the target image in a separate `if`
///   that does not touch the insert loop, so every pane the packer placed still gets its cell on
///   the repack frame;
/// - a pane that **did not fit the capped atlas** gets no cell at all ([`shelf_pack`] returns a
///   prefix of its input) — and therefore pushes no quad, so freezing its cloud is the right
///   answer rather than a dropped frame.
fn draws_this_frame(bridge: &UiModelTiles, handle: &FrameHandle) -> bool {
    bridge.cells.contains_key(handle)
}

/// [`compose_tiles`]'s pure half: the quads for every `(request, cell)` pair the bridge holds,
/// ordered by paint key so the overlay diff sees the same sequence for the same set.
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
                // The instance draws at the widget's OWN alpha (`0x76d120`).
                color: [1.0, 1.0, 1.0, req.alpha],
                // A render target: premultiplied by construction (`UiQuad` doc).
                premultiplied: true,
                clip: req.clip,
                ..default()
            })
        })
        .collect();
    out.sort_by_key(|q| q.z_key);
    out
}

/// The engine's facts for a resident file: its sequence table and header bounds.
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

/// Shelf-pack `sizes` (with a gutter) into the smallest power-of-two square atlas from
/// [`ATLAS_MIN`] to [`ATLAS_MAX`] that fits; returns the cells (one per size that fit, in order)
/// and the atlas size chosen (`0×0` for no sizes).
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
    /// The tile's cell-clip row — see [`Tile::clip_slot`].
    clip_slot: Option<u16>,
    alpha_parts: Vec<AlphaPart>,
    uv_parts: Vec<UvPart>,
    emitters: Vec<Entity>,
}

/// Spawn a file's parts, rig and emitters under `root` on the tile layer — the booth bake's
/// recipe (`portrait::booth::spawn_booth_model`) for a file with no unit. `None` when a material
/// is not resident yet (the caller retries next frame rather than latch a world-lit twin).
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
    // The render forms, now (the booth/marker lanes' exception to the paced furnisher: one small
    // model, on demand).
    forms.ensure_now_rigged(handle, &model.submeshes, meshes);
    let built = forms.slices(handle);
    let (stat_forms, skin_forms) = (built.stat, built.skin.unwrap_or(&[]));

    // The tile's **cell clip** row (decision 2093): one row per tile, `anim_slots.w` on every
    // one of its materials, the cell rect written into it each frame. It is why a tile's batches
    // are all its OWN clones rather than the shared twins — the twin is per (material, light),
    // and the clip is per PANE.
    let clip_slot = render.table.alloc();
    // Materials first — every one must be resident before anything spawns, or a retry would
    // leave half a tree behind.
    let mut part_mats: Vec<Handle<WowModelMaterial>> = Vec::with_capacity(model.submeshes.len());
    let mut uv_parts: Vec<UvPart> = Vec::new();
    for (i, sub) in model.submeshes.iter().enumerate() {
        let texture = if sub.icon_slot {
            icon_tex.clone()
        } else {
            sub.texture.clone()
        };
        let world = render.mats.steady(sub, texture, (i + 1) as u16)?;
        // The twin: same material, this rig's light buffer, and the batch's AUTHORED fog policy
        // when the pane armed fog — which is where the per-material UNFOGGED bit does its own
        // work (`0x70bb24`: a fogged pane still draws its unfogged materials unfogged). A
        // pane with no fog forces it off, as every tile did before 2027. The shade selector both
        // lanes flip is inert on an unlit batch, which is every shipped UI M2.
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
        // A batch whose texture transform animates draws through a clone of its own, with its
        // own table rows — the rows are written off THIS pane's play head, so two panes on one
        // file cannot share them (decision 2019).
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

    // The rig: the collapsed pose buffer + a palette slot, when the file has bones. The tile
    // camera is not the world camera, so bone billboards are left to the rest pose (none of the
    // shipped UI files authors one).
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
            // A paused player: the engine's play head seeks it every frame (2007's clock).
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

    // The emitters: on their bone's anchor (the collapsed rig's demand-spawned entity), or the
    // root for a boneless file; clocked by the root's player like any hosted cloud; lit by the
    // tile's buffer; sized in the tile's pixels (`set_size_scale`, written per frame).
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
    // `spawn_anim_host` is the world's placement recipe (variation re-rolls, the residency
    // window); a widget arms exactly what Lua asked and nothing else, so it is not used here —
    // named so nobody reaches for it.
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

    /// The packer keeps every cell inside the atlas, gutters between, and answers cells in
    /// request order — the composite's UVs and the roots' placements both index by it.
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
        // Growth: a wall of big tiles needs a bigger atlas; an impossible one is capped and
        // the tail simply does not fit.
        let big: Vec<UVec2> = (0..8).map(|_| UVec2::new(400, 400)).collect();
        let (cells, atlas) = pack(&big);
        assert_eq!(cells.len(), 8);
        assert_eq!(atlas, UVec2::splat(2048));
        let huge = vec![UVec2::new(5000, 10)];
        let (cells, atlas) = pack(&huge);
        assert!(cells.is_empty());
        assert_eq!(atlas, UVec2::splat(ATLAS_MAX));
    }

    /// The composite is a function of the bridge alone (decision 2023): a request with no cell
    /// draws nothing, a cell draws its request's rect at the request's key and alpha with the
    /// cell's texel window, and a cell whose request is gone draws nothing — no extract in the
    /// loop.
    #[test]
    fn the_composite_is_the_bridges_cells_over_their_requests() {
        // Two live handles off a real arena — the bridge is keyed by them, nothing more.
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

    /// **The park verdict and the composite are the same question** (decision 2046). A tile
    /// parks exactly when it pushed no quad, and `sync_tiles` reads both off `bridge.cells` in one
    /// pass — so neither an atlas repack nor an atlas too full for one more pane can freeze a
    /// cloud on a frame its pane is visibly drawing. This pins the two sides to each other: what
    /// [`composite_quads`] draws IS the un-parked set, so a later change to either has to change
    /// both. The third pane here is the one the capped atlas had no room for: it draws nothing,
    /// which is exactly why parking it is right.
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
        // And the repack: growing the atlas moves every cell's texel window without changing WHO
        // has one, so no pane's verdict flips on a growth frame.
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

    /// One freshly built VM with a shown cooldown pane on it — the shape `Cooldown.xml` builds
    /// per action button, and the shape `!OmniCC` wraps `CooldownFrame_SetTimer` around.
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

    /// **A rebuilt VM is told about a file the host already loaded** — decision 1290's class,
    /// found in the tile renderer's facts cache.
    ///
    /// `UiScript::model_facts_wanted` DRAINS, and the only thing that re-pushes a want is
    /// `SetModel`; `visible_model_panes` drops a pane whose file the engine holds no facts for.
    /// So an answer skipped once is skipped for that VM's whole life. The host used to skip it
    /// whenever the ASSET was resident — a fact about the process, not about the VM — and the VM
    /// is rebuilt at every logout, login and `ReloadUI()`. From the second world entry on, every
    /// `<Model>` widget in the game (the cooldown pie and the GCD sweep, the autocast shine, the
    /// minimap and world-map pings, the item-push card, the map arrow) drew nothing at all, and
    /// only restarting the client brought them back.
    #[test]
    fn a_rebuilt_vm_is_told_about_a_file_the_host_already_loaded() {
        let key = benilla_ui::widget::model_key(COOLDOWN_FILE);
        let mut state = TileState::default();

        // Session 1 — nothing is resident, so the host is asked to load the file.
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

        // Session 2 — the logout/login (or `/reload`) rebuild: a fresh VM, the same file.
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

    /// **A new VM inherits nothing keyed by a `FrameHandle`** — the same edge, its other half.
    ///
    /// A handle is a generational index into ONE arena; the next VM reissues the same indices at
    /// the same generations, so a surviving tile or request does not go stale, it silently
    /// re-attaches to a different frame. The tile's tree is despawned and its mat-anim rows go
    /// back to the table, exactly as a reaped tile's do.
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

    /// A `TileRequest` for the perspective tests: the pane's own size and root terms, nothing
    /// the leg does not read.
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

    /// `HumanMale`'s camera 1 as `benilla-extract m2cam` reads it — and as the reference records
    /// it, to the digit.
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

    /// The perspective leg's projection is the client's `0x5c3cc0`: a **diagonal** fov, so
    /// `t = tan(fovy / (2·√(aspect²+1)))`, `m11 = 1/t`, `m00 = m11/aspect` — not a vertical fov,
    /// and not an aspect-independent crop.
    ///
    /// The numbers are the reference's own worked checks: a `318×224` pane gives
    /// `θ = 0.287938 · fov` and a `233×224` pane `θ = 0.346523 · fov`. The fallback camera's check:
    /// `aspect = 1.4196429`, `√(aspect²+1) = 1.7364815`, `tan(0.5/(2·1.7364815)) = 0.1449700`.
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
        // `m11 = 1/t` with `t = tan(θ)`; the fallback check's `t` is 0.1449700.
        let t = 1.0 / m.y_axis.y;
        assert!((t - 0.144_97).abs() < 1e-5, "t = {t}");
        // …and `m00 = m11 / aspect` — the one `aspect` doing both jobs, never 1.0 (a transplanted
        // 1.0 would stretch a sphere 1.42× wider than tall in this pane).
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

    /// **`SetModelScale` and `SetPosition` CANCEL on the perspective leg** (`0x718960`): the
    /// authored eye and target are carried through the very root transform the geometry is drawn
    /// through, so the picture is invariant to both. This is the property the leg is built on, and
    /// the failure mode it guards is applying the root to the geometry but not to the camera,
    /// which is wrong by `1/s`.
    ///
    /// The check is on the pixels, not on the matrices: a model-local point's clip-space `x/w`
    /// and `y/w` must be identical at any scale and any offset.
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

        // …and the ORTHOGRAPHIC leg is the opposite: its scale is a pixel ladder with no camera
        // to cancel against, so `px_per_unit` is exactly what the model's size is measured in.
        // (Stated as the contrast, so nobody carries the cancellation across the fork.)
        assert_ne!(
            persp_req(UVec2::new(318, 224), 1.0, Vec3::ZERO, 0.0).px_per_unit,
            0.0
        );
    }

    /// **`SetFacing` cancels too** (on `<PlayerModel>`'s FROZEN camera, `0x7acf10`, only
    /// `SetFacing`/`SetRotation` show; that does not hold here).
    ///
    /// The reason is the up vector: `0x7ac640` builds it as `(0, −sin(roll), cos(roll))` in model
    /// space out of `CCamera` fields the publish never writes, and at `roll = 0` — every camera a
    /// `<Model>` pane can name — that is model-space `+Z`, the very axis `SetFacing` turns about.
    /// Eye, target, geometry and up all turn together, so the image does not move.
    ///
    /// A re-implementation that rotated the model without rotating its camera would spin the
    /// model; one that rotated both but kept a screen-space up would tilt it. Both are wrong, and
    /// both look plausible, which is why this is pinned on the pixels.
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
        // The ORTHO leg is where a facing shows: there the model turns in the screen plane and
        // nothing turns with it. Same widget field, opposite outcome — the fork is the point.
        let turned = wow_to_screen()
            * Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
            * benilla_assets::coords::wow_to_bevy([1.0, 0.0, 0.0]);
        assert!((turned - Vec3::Y).length() < 1e-5, "{turned}");
    }

    /// The camera's eye lands where the record says, in Bevy space, and the view looks down the
    /// eye→target axis with the model's up (WoW `+Z`) up — the `lookAt` half of the leg, checked
    /// against `HumanMale`'s camera 1: the eye is 4.02 model units in front of the target on the
    /// model's own `+X`, at chest height.
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
        // The distance the record authors — the whole mechanism behind "the pane looks
        // normalized" (measured over the shipped models: Blizzard authored a per-model distance).
        let d = (cam.target - cam.eye).length();
        assert!((d - 4.0234).abs() < 1e-3, "authored eye distance {d}");
        // Up is the model's own up, not the camera's roll-free default in some other frame.
        assert!(leg.camera.up().as_vec3().dot(Vec3::Y) > 0.9);
    }

    /// The axis fix is a proper rotation that puts WoW `+X` right, `+Y` up, `+Z` toward the
    /// viewer.
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
