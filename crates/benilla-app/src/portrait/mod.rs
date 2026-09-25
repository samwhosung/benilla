//! Unit-frame portraits and the full-body model panes, baked off-screen.
//!
//! The 1.12 client renders a unit's model once into a 64² texture, frozen until the model
//! changes, and stamps a round stencil into it (`0x524f60`). A booth here bakes the same still and
//! the UI cuts the circle at draw time. The body panes run the same pipeline, square at 512²,
//! framed through the model's `<PlayerModel>` camera ([`framing::body_frame`]) and spun to a yaw.
//!
//! A booth mirrors the unit's live dressed look ([`PortraitPart`]), so a gear change re-bakes;
//! until the model loads the slot shows a 2D stand-in, where the reference leaves it blank
//! (`0x519fcc`) and loads a stand-in only for a unit it holds no object for (`0x525ba0`).
//! Framing is the model's
//! authored portrait camera, the one `cameraLookup[0]` selects (`0x713540`), through the
//! diagonal-FOV projection ([`WowPortraitProjection`]); a camera-less model falls back to
//! [`frame`]. The bake is a fresh instance frozen at Stand (`0x707400`, `0x7121a0`).
//!
//! Deviation: the round portraits bake at 256², not 64², because the 64² still is a resolution
//! limit, not a look; and under a fixed studio light, not the ambient state, because that neutral
//! light is the chosen portrait look (`light::BoothLight`).
//!
//! The frozen Stand phase is t=0; the reference's sampling clock is untraced.
//!
//! `WOW_PORTRAIT_TEST=<Model\Path.mdx>` (with `WOW_PORTRAIT_TEST_SKIN=<blp>`) bakes that model into
//! every slot without a server.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{PerspectiveProjection, Projection, RenderTarget};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::Msaa;

use crate::entities::Creatures;
use crate::net::{NetEntity, SelfPlayer};
use crate::target::Selection;
use benilla_assets::materials::WowModelMaterial;

mod framing;
pub(crate) use framing::{
    attachment_point, glue_box_aspect, glue_canvas_bars, head_anchor, pane_projection,
    PortraitAnchors, WowPortraitProjection,
};
use framing::{body_frame, frame, PORTRAIT_FOV};
mod booth;
/// Translucency twins, one type for the preview and booth halves of a batch so they agree.
pub(crate) use booth::BoothTwins;
use booth::{
    clear_booth_rig, spawn_booth_effects, spawn_booth_model, spawn_booth_own_emitters,
    BoothBillboardSpec, BoothEffects, BoothInstance, BoothMotion, BoothPart, BoothRider,
};
mod dressup;
pub(crate) use dressup::{DressUpBake, DressUpLook, DressUpPreview};
mod glue_booth;
pub(crate) use glue_booth::{
    CreateLook, GhostKit, GlueLook, GluePetBake, GluePreview, GluePreviewBake, GlueScene, PetLook,
    PreviewBillboard, PreviewEffects, PreviewPart, PreviewRider, SelectLook, GLUE_SLOT,
};
mod light;
pub(crate) use light::{material_variant, VariantLane};
use light::{model_pane_light, studio_light, BoothLight};
pub(crate) mod test_bake;

/// The round portrait slots, each with its own layer and camera. `"npc"` is an interaction
/// window's NPC ([`crate::ui_session::InteractNpc`]); `"targettarget"` resolves only while drawn.
const SLOTS: [&str; 9] = [
    "player",
    "target",
    "targettarget",
    "pet",
    "npc",
    "party1",
    "party2",
    "party3",
    "party4",
];
/// The character window's full-body pane: the dressed player, sampled square.
const PAPERDOLL_SLOT: &str = "paperdoll";
/// The inspect window's full-body pane: the paper doll's bake pointed at another player.
const INSPECT_SLOT: &str = "inspect";
/// The pet paper doll's full-body pane, a booth of its own because the `"pet"` slot is a 256²
/// bust with no yaw.
const PETDOLL_SLOT: &str = "petdoll";
/// The stable window's pane (`0x4cb870`), whose subject may be a stabled pet with no world object
/// ([`StableBooth`]).
const STABLE_SLOT: &str = "stable";

/// Which booth a `<Model>`/`<PlayerModel>` pane samples, keyed by the pane's global name from the
/// stock file (`PaperDollFrame.xml`, `TabardFrame.xml`, `PetPaperDollFrame.xml`,
/// `InspectPaperDollFrame.xml`, `PetStable.xml`, `DressUpFrame.xml`,
/// `Blizzard_AuctionDressUp.xml`). A pane not in this table draws nothing.
const MODEL_PANE_BOOTHS: [(&str, &str); 7] = [
    ("CharacterModelFrame", PAPERDOLL_SLOT),
    // The tabard designer's pane: the paper doll's bake, wearing the design under preview.
    ("TabardModel", PAPERDOLL_SLOT),
    ("PetModelFrame", PETDOLL_SLOT),
    ("InspectModelFrame", INSPECT_SLOT),
    ("PetStableModel", STABLE_SLOT),
    ("DressUpModel", dressup::DRESSUP_SLOT),
    // Deviation: the auction house's dressing room shares the one dressing-room booth, because the
    // app's `TryOn`/`Dress` intents are not per-widget; the reference's two widgets each clone
    // their own model.
    ("AuctionDressUpModel", dressup::DRESSUP_SLOT),
];

/// The booth a named model pane samples, or `None` for a pane no window has claimed.
pub(crate) fn model_pane_booth(name: &str) -> Option<&'static str> {
    MODEL_PANE_BOOTHS
        .iter()
        .find(|(pane, _)| *pane == name)
        .map(|(_, slot)| *slot)
}

/// World is layer 0, the UI pass layer 1; each portrait slot gets its own layer from here up.
const PORTRAIT_LAYER_BASE: usize = 2;
/// The paper-doll booth's layer, the first past the portrait slots.
const PAPERDOLL_LAYER: usize = PORTRAIT_LAYER_BASE + SLOTS.len();
const INSPECT_LAYER: usize = PAPERDOLL_LAYER + 1;
const PETDOLL_LAYER: usize = INSPECT_LAYER + 1;
const STABLE_LAYER: usize = PETDOLL_LAYER + 1;
/// The glue booth's layer. Every booth layer is computed in this one ladder: two cameras on one
/// layer collide silently, in rendering and in the emitter-to-camera match (`particles::sim` takes
/// the first camera whose layers intersect).
pub(super) const GLUE_LAYER: usize = STABLE_LAYER + 1;
/// The dressing room's layer.
pub(super) const DRESSUP_LAYER: usize = GLUE_LAYER + 1;
/// The warm pass's twin booth layer; only the warm menagerie rides it, only while the pass runs
/// ([`spawn_warm_booth`]).
pub(crate) const WARM_BOOTH_LAYER: usize = DRESSUP_LAYER + 1;
/// The warm pass's orthographic twin camera layer: bevy_pbr keys pipelines on the projection
/// class, and the tile atlas camera is a third class.
pub(crate) const WARM_ORTHO_LAYER: usize = WARM_BOOTH_LAYER + 1;
/// The minimap interior composite's layer: not a booth, but an offscreen camera on the ladder.
pub(crate) const MINIMAP_COMPOSITE_LAYER: usize = WARM_ORTHO_LAYER + 1;
/// The UI model tiles' layer (`crate::ui_models`): every `<Model>` widget's M2 renders into one
/// atlas through one camera on it.
pub(crate) const UI_MODELS_LAYER: usize = MINIMAP_COMPOSITE_LAYER + 1;
/// The first perspective model pane layer: a `<Model>` framed by its own camera needs a camera,
/// and so a layer, of its own. The block tops the ladder so it can widen.
pub(crate) const UI_MODEL_CAM_LAYER_BASE: usize = UI_MODELS_LAYER + 1;
/// How many perspective model panes draw at once; a ninth draws nothing.
pub(crate) const UI_MODEL_CAM_LAYERS: usize = 8;

// A booth camera's layer is its identity, for rendering and for the emitter-to-camera match.
const _: () = assert!(
    PAPERDOLL_LAYER != INSPECT_LAYER
        && INSPECT_LAYER != PETDOLL_LAYER
        && PETDOLL_LAYER != STABLE_LAYER
        && STABLE_LAYER != GLUE_LAYER
        && PAPERDOLL_LAYER != STABLE_LAYER
        && INSPECT_LAYER != STABLE_LAYER
        && PETDOLL_LAYER != GLUE_LAYER
        && PAPERDOLL_LAYER != PETDOLL_LAYER
        && PAPERDOLL_LAYER != GLUE_LAYER
        && INSPECT_LAYER != GLUE_LAYER
        && DRESSUP_LAYER > GLUE_LAYER
        && WARM_BOOTH_LAYER > DRESSUP_LAYER
        && WARM_ORTHO_LAYER > WARM_BOOTH_LAYER
        && MINIMAP_COMPOSITE_LAYER > WARM_ORTHO_LAYER
        && UI_MODELS_LAYER > MINIMAP_COMPOSITE_LAYER
        && UI_MODEL_CAM_LAYER_BASE > UI_MODELS_LAYER,
    "booth render layers must be distinct — see GLUE_LAYER"
);
const _: () = assert!(
    PAPERDOLL_LAYER > PORTRAIT_LAYER_BASE + SLOTS.len() - 1,
    "booth layers must not overlap the per-slot portrait layers"
);
/// The round portraits' bake size, square (the reference bakes 64²); `ui_quad.wgsl`'s `circular`
/// cuts the circle at draw time.
const PORTRAIT_SIZE: u32 = 256;
/// The body panes' bake size: covers the ~233×224-point pane at 2× hidpi.
const PAPERDOLL_SIZE: u32 = 512;
/// Stamped by the attach path on every unit-model part child: the part's skinned and static
/// bind-pose mesh twins and its steady exterior material, which a booth mirrors.
#[derive(Component)]
pub(crate) struct PortraitPart {
    pub(crate) static_mesh: Handle<Mesh>,
    /// `None` for a WMO-display part, which never skins; the booth draws the static twin.
    pub(crate) skinned_mesh: Option<Handle<Mesh>>,
    pub(crate) material: Handle<WowModelMaterial>,
}

/// Stamped on every bone-rider mesh child (helm, shoulder, held item). A booth seats it under its
/// own skeleton's joint, as the reference resets the attach sockets (`0x47a230`).
#[derive(Component)]
pub(crate) struct PortraitRider {
    pub(crate) static_mesh: Handle<Mesh>,
    pub(crate) material: Handle<WowModelMaterial>,
    /// The body-skeleton bone the rider's joint entity belongs to.
    pub(crate) bone: u16,
    /// The attach point's Bevy-space offset under that bone ([`crate::entities::BoneAttach`]).
    pub(crate) offset: Vec3,
    /// The M2 attachment id the rider hangs from, for [`attach_reset`]; `None` for the host
    /// model's own geometry, which sits in no attachment node.
    pub(crate) attach: Option<u16>,
}

/// Stamped on an anchor child for each camera-facing batch a unit wears (the eye-glow, an item's
/// gem or halo, an item glow), so a booth can rebuild it: the world card is root-spawned.
#[derive(Component)]
pub(crate) struct PortraitBillboard {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) material: Handle<WowModelMaterial>,
    /// The body bone whose booth joint the card seats on: the eye bone, or the item's attach bone.
    pub(crate) bone: u16,
    pub(crate) seat: PortraitSeat,
    pub(crate) kind: benilla_formats::BillboardKind,
    /// The M2 attachment id, for [`attach_reset`]; `None` for the host model's own batch (the
    /// eye-glow), which the reset's attachment walk never reaches.
    pub(crate) attach: Option<u16>,
}

/// Whose model a mirrored camera-facing batch belongs to, which decides its pivot and whether a
/// mounted unit's booth keeps it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum PortraitSeat {
    /// A batch of the rigged host model (the eye-glow): its joint frame already bakes the pivot,
    /// and a mount's own card prunes with the mount.
    Body,
    /// A batch of a rig-less rider (an item, an item glow), at the attach point plus its own
    /// pivot. Kept whatever the mount state: mounted, the character's joints re-root inside the
    /// mount.
    Rider(Vec3),
}

impl PortraitSeat {
    /// The offset a booth seats this batch at under its bone's joint.
    pub(crate) fn offset(self) -> Vec3 {
        match self {
            PortraitSeat::Body => Vec3::ZERO,
            PortraitSeat::Rider(at) => at,
        }
    }
}

/// Stamped on an effect-bearing model riding a unit (an item's emitters, a weapon's `ItemVisuals`
/// glow), carrying the emitter records for a body booth to spawn its own copies.
#[derive(Component)]
pub(crate) struct PortraitEffects {
    /// The body bone whose booth joint the effect model's host seats on.
    pub(crate) bone: u16,
    /// The M2 attachment id, for [`attach_reset`]; always `Some`, as every publisher is an item.
    pub(crate) attach: Option<u16>,
    /// The host's offset in the bone's joint frame: the attach point, plus the glow slot's offset.
    pub(crate) offset: Vec3,
    pub(crate) emitters: Vec<benilla_assets::ModelEmitter>,
}

/// A bodiless subject built from the display cache only to be mirrored: how a body booth draws a
/// stabled pet, which has no world object. Carries the display id a unit reads off its wire record.
#[derive(Component)]
pub(crate) struct PortraitStandIn(pub(crate) u32);

/// What a portrait slot shows: the booth's bake, or a 2D stand-in file, for a unit with no object
/// (`0x525ba0`) and while a unit's model streams in (the reference shows blank then, `0x519fcc`).
#[derive(Clone, PartialEq)]
pub(crate) enum PortraitSource {
    /// The slot's off-screen render target (the model bake).
    Live(Handle<Image>),
    /// A flat portrait BLP (`Interface\CharacterFrame\TemporaryPortrait-…`), resolved by the UI
    /// extract through the standard sprite path.
    File(String),
}

/// The GUID-keyed bake cache (`0xc0ce7c`): `SetPortraitTexture` on a player the client does not
/// hold binds the cached bake on a hit (`0x525c36` → `0x770300`) and loads the stand-in only on a
/// miss, so a party member who walks out of range keeps their face. A booth losing its member
/// hands its target in here ([`sync_portraits`]).
///
/// Deviation: only the four party slots hand over, capped at [`Self::CAP`], instead of every
/// portrait until `ClientDestroyGame` (`0x401ee0`), because a general cache would pay a texture
/// per unit ever seen.
#[derive(Resource, Default)]
pub(crate) struct PortraitBakes {
    faces: HashMap<u64, Handle<Image>>,
    order: Vec<u64>,
}

impl PortraitBakes {
    /// A full party, plus slack for a churning roster.
    const CAP: usize = 8;

    /// The bake standing for `guid`: the probe `0x525ba0` makes before the stand-in.
    fn get(&self, guid: u64) -> Option<Handle<Image>> {
        self.faces.get(&guid).cloned()
    }

    /// Take a booth's target as `guid`'s face. A re-bake replaces in place, as the reference
    /// re-stores the handle at `+0x24`; only a new guid can evict.
    fn store(&mut self, guid: u64, face: Handle<Image>) {
        if self.faces.insert(guid, face).is_none() {
            self.order.push(guid);
            if self.order.len() > Self::CAP {
                let oldest = self.order.remove(0);
                self.faces.remove(&oldest);
            }
        }
    }
}

/// Unit token to what its portrait region shows: the booth writes it, and the UI extract
/// ([`crate::ui_script`]) reads it for a `SetPortraitTexture`-bound region.
#[derive(Resource, Default)]
pub(crate) struct PortraitImages(pub(crate) HashMap<String, PortraitSource>);

/// The paper-doll pane's yaw in radians, `Model:SetRotation`'s convention (rotate-left decrements;
/// `0.61` is `Model_OnLoad`'s default, `UIParent.lua:1422`). The booth re-poses only on a change.
#[derive(Resource)]
pub(crate) struct PaperDollBooth {
    pub(crate) yaw: f32,
}

impl Default for PaperDollBooth {
    fn default() -> Self {
        Self { yaw: 0.61 }
    }
}

/// The inspect pane's yaw and unit ([`crate::ui_inspect`]'s resolve); `None` empties the booth.
#[derive(Resource)]
pub(crate) struct InspectBooth {
    pub(crate) yaw: f32,
    pub(crate) unit: Option<Entity>,
}

impl Default for InspectBooth {
    fn default() -> Self {
        Self {
            yaw: 0.61,
            unit: None,
        }
    }
}

/// The pet paper doll's yaw and unit ([`crate::ui_pet_doll`]'s resolve); `None` empties the booth.
#[derive(Resource)]
pub(crate) struct PetDollBooth {
    pub(crate) yaw: f32,
    pub(crate) unit: Option<Entity>,
}

impl Default for PetDollBooth {
    fn default() -> Self {
        Self {
            yaw: 0.61,
            unit: None,
        }
    }
}

/// The stable pane's input. `SetPetStablePaperdoll` resolves the summoned pet by GUID (`unit`,
/// when out and streamed) and any other through the creature cache (`display_id`; a dismissed pet
/// falls to `0xb72060`, the summoned record's entry). Both `None` empties the booth. No size ramp:
/// `0x4cb870` and `0x505cb0` write no scale, and `0x505a70` applies only the texture variation
/// (`0x4797b0`).
#[derive(Resource)]
pub(crate) struct StableBooth {
    pub(crate) yaw: f32,
    pub(crate) unit: Option<Entity>,
    pub(crate) display_id: Option<u32>,
}

impl Default for StableBooth {
    fn default() -> Self {
        Self {
            // `Model_OnLoad`'s default facing (`UIParent.lua:1422`).
            yaw: 0.61,
            unit: None,
            display_id: None,
        }
    }
}

/// What a booth has baked; any change to the unit's dressed look re-bakes.
#[derive(PartialEq)]
struct LookKey {
    draws: Vec<(AssetId<Mesh>, AssetId<WowModelMaterial>)>,
    /// Per mirrored effect: bone, offset bits, emitter count. An item glow lands after its meshes.
    effects: Vec<(u16, [u32; 3], usize)>,
}

impl LookKey {
    /// The key for one mirrored dressed look, in the order [`DressedLook::collect`] returns it.
    fn build(
        parts: &[&PortraitPart],
        riders: &[&PortraitRider],
        billboards: &[&PortraitBillboard],
        effects: &[&PortraitEffects],
    ) -> Self {
        LookKey {
            draws: parts
                .iter()
                .map(|p| (p.static_mesh.id(), p.material.id()))
                .chain(riders.iter().map(|r| (r.static_mesh.id(), r.material.id())))
                .chain(billboards.iter().map(|b| (b.mesh.id(), b.material.id())))
                .collect(),
            effects: effects
                .iter()
                .map(|e| {
                    (
                        e.bone,
                        e.offset.to_array().map(f32::to_bits),
                        e.emitters.len(),
                    )
                })
                .collect(),
        }
    }
}

/// One booth: its render layer, model root, render target and the look it has baked.
struct Booth {
    layer: RenderLayers,
    root: Entity,
    target: Handle<Image>,
    baked: Option<LookKey>,
    /// Whose face the bake is, for the handover into [`PortraitBakes`] (party slots only).
    baked_guid: Option<u64>,
    /// Body panes only: the last snapshot, in place of `baked` ([`SnapKey`]).
    snap: Option<SnapKey>,
    /// The pane was drawn last frame: the edge detector for the show override (`0x505d00`).
    shown: bool,
    show_rev: u32,
    /// Frames the camera stays active after a content edge; at 0 with nothing pending it sleeps
    /// and the target keeps the last render.
    wake: u32,
    /// A live widget (a body pane): it renders every frame its pane is drawn ([`BoothPanes`]).
    live: bool,
    /// Textures the bake referenced that were not yet resident; the camera waits for each.
    pending: Vec<Handle<Image>>,
    /// When the `pending` hold began (wall secs), released at [`PENDING_LANDING_SECS`].
    pending_since: Option<f64>,
    /// The bake has not yet drawn with every pipeline compiled: off macOS a batch whose variant
    /// is still building is skipped silently, and a still that slept then would keep the hole.
    pipes_settling: bool,
    /// When the `pipes_settling` hold began (wall secs), bounded at [`PIPELINE_SETTLING_SECS`].
    pipes_since: Option<f64>,
    /// The pane aspect the camera is framed for: 1.0 until first drawn, then sticky.
    aspect: f32,
    /// A rig stands in this booth, for the park gate.
    rigged: bool,
    turn: Turn,
    /// The scene is parked: its camera sleeps and [`benilla_world::rig_anim::AnimParked`] holds
    /// the pose; its emitters freeze, as the reference ticks only an emitter its frame draws.
    parked: bool,
}

/// A body pane's turn animation. `PlayerModel:SetRotation` (`0x505bb0`) picks a turn-in-place
/// shuffle by direction, queues it with a 100 ms expiry, then writes the facing.
#[derive(Default)]
struct Turn {
    /// The facing the root is posed at (`[+0x39c]`); `None` until the first pose, so a fresh bake
    /// does not step.
    faced: Option<f32>,
    /// This frame's rotation: the id [`booth::turn_shuffle`] picked, taken by the driver.
    spun: Option<u16>,
    /// The shuffle stepping and the wall-clock second it expires; `None` on Stand.
    shuffle: Option<(u16, f64)>,
    /// The node the primary runs: every arm rolls a weighted variation (`0x7121a0`'s `-1`).
    playing: Option<bevy::animation::graph::AnimationNodeIndex>,
    fade: Option<Fade>,
}

impl Turn {
    /// The bake replaced the `AnimationPlayer`: drop its node references. [`Self::faced`]
    /// survives, since a re-bake is `RefreshUnit`, which never calls `SetRotation`.
    fn rebaked(&mut self) {
        *self = Turn {
            faced: self.faced,
            ..Default::default()
        };
    }
}

/// The cross-fade out of the pose an arm replaced, the reference's secondary blend slot. Both
/// turn call sites (`0x505c23`, `0x505c98`) arm it through `0x7121a0`, and `0x7125d6` copies the
/// live primary track, still on its own clock, into `[blk+0xc4]`; `0x714880` blends by a
/// smoothstep λ. One slot: a new arm overwrites it.
#[derive(Clone, Copy)]
struct Fade {
    node: bevy::animation::graph::AnimationNodeIndex,
    /// The wall-clock second λ reaches 0 (`[blk+0x100]`).
    until: f64,
    /// The window width: the incoming clip's `M2Sequence.blendTime` (`0x7125f2`).
    span: f32,
}

/// Frames a content edge keeps a booth camera rendering (spawn, re-face lag, upload).
const BOOTH_SETTLE_FRAMES: u32 = 4;

/// A rig on a booth stage: its `AnimParked` marker belongs to [`gate_booth_cameras`] alone, and
/// the world-view parker skips it, since a stage is outside every world frustum.
#[derive(bevy::prelude::Component)]
pub(crate) struct StageRig;

/// How long a [`Booth::pending`] hold may keep the camera awake (wall secs).
const PENDING_LANDING_SECS: f64 = 10.0;

/// How long a [`Booth::pipes_settling`] hold may keep the camera awake (wall secs).
const PIPELINE_SETTLING_SECS: f64 = 15.0;

/// `WOW_BOOTH_LOG=1`: is the booth instrument armed (read once)?
fn booth_log() -> bool {
    static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG.get_or_init(|| std::env::var_os("WOW_BOOTH_LOG").is_some())
}

/// `WOW_BOOTH_LOG=1`: one line per bake decision, with the counts and attach ids it saw.
fn log_bake(
    token: &str,
    verb: &str,
    parts: &[&PortraitPart],
    riders: &[&PortraitRider],
    billboards: &[&PortraitBillboard],
    effects: &[&PortraitEffects],
) {
    if booth_log() {
        // The attach ids the bake is dressed at: no id `attach_reset` cuts may appear.
        let mut at: Vec<u16> = riders
            .iter()
            .filter_map(|r| r.attach)
            .chain(billboards.iter().filter_map(|b| b.attach))
            .chain(effects.iter().filter_map(|f| f.attach))
            .collect();
        at.sort_unstable();
        at.dedup();
        eprintln!(
            "[booth] {token} {verb} parts={} riders={} billboards={} fx={}/{} at={at:?} grip={:?}",
            parts.len(),
            riders.len(),
            billboards.len(),
            effects.len(),
            effects.iter().map(|e| e.emitters.len()).sum::<usize>(),
            hand_grip(riders, billboards, effects),
        );
    }
}

/// The framing anchors for a bake; `None` means the display is still loading, retry next frame.
/// A bake on made-up zero anchors would aim wrong and latch.
fn booth_anchors(
    creatures: Option<&Creatures>,
    display_id: Option<u32>,
) -> Option<PortraitAnchors> {
    let Some(display_id) = display_id else {
        return Some(PortraitAnchors {
            camera: None,
            pane_camera: None,
            bbox_center: Vec3::ZERO,
            head: None,
            pivot_height: 0.0,
            ground_radius: 0.0,
        });
    };
    creatures?.display_anchors(display_id)
}

/// `WOW_BOOTH_LOG=1`: the resolved framing for a bake and where the camera ended up.
fn log_frame(token: &str, a: &PortraitAnchors, cam: &Transform) {
    if booth_log() {
        eprintln!(
            "[booth] {token} frame eye=({:.3},{:.3},{:.3}) authored_cam={} pivot={:.3} head_y={:.3} gr={:.3}",
            cam.translation.x,
            cam.translation.y,
            cam.translation.z,
            a.camera.is_some(),
            a.pivot_height,
            a.head.map_or(f32::NAN, |h| h.y),
            a.ground_radius,
        );
    }
}

/// `WOW_BOOTH_LOG=1`: each booth's posed skeleton extent, logged when it moves.
fn log_booth_pose(
    booths: Res<Booths>,
    rigs: Query<&benilla_world::rig_anim::RigPose>,
    mut last: Local<HashMap<String, [i32; 6]>>,
) {
    if !booth_log() {
        return;
    }
    for (token, booth) in &booths.0 {
        let Ok(pose) = rigs.get(booth.root) else {
            last.remove(token);
            continue;
        };
        let (mut mn, mut mx) = (Vec3::MAX, Vec3::MIN);
        for m in &pose.model {
            let t = Vec3::from(m.translation);
            mn = mn.min(t);
            mx = mx.max(t);
        }
        let cm = |v: Vec3| [v.x, v.y, v.z].map(|c| (c * 100.0).round() as i32);
        let ([a, b, c], [d, e, f]) = (cm(mn), cm(mx));
        let key = [a, b, c, d, e, f];
        if last.insert(token.clone(), key) == Some(key) {
            continue;
        }
        eprintln!(
            "[booth] {token} posed-bones n={} min=({:.3},{:.3},{:.3}) max=({:.3},{:.3},{:.3})",
            pose.model.len(),
            mn.x,
            mn.y,
            mn.z,
            mx.x,
            mx.y,
            mx.z,
        );
    }
}

/// Arm `booth` after a content edge: the settle window, plus every frame until each of `twins`'s
/// textures is resident.
fn wake_booth<'a>(
    booth: &mut Booth,
    mats: &Assets<WowModelMaterial>,
    twins: impl Iterator<Item = &'a Handle<WowModelMaterial>>,
) {
    booth.wake = BOOTH_SETTLE_FRAMES;
    booth.pending = twins
        .filter_map(|h| mats.get(h))
        .filter_map(|m| m.base.base_color_texture.clone())
        .collect();
    booth.pending_since = None;
    // A fresh bake owes a pipeline settle, judged once the settle window drains.
    booth.pipes_settling = true;
    booth.pipes_since = None;
}

#[derive(Resource, Default)]
struct Booths(HashMap<String, Booth>);

/// Where each booth is sampled on screen this frame: slot token to the region's aspect, from the
/// UI extract. A body pane frames at that aspect and renders only while drawn; one frame stale,
/// so the aspect latches into [`Booth::aspect`].
#[derive(Resource, Default)]
pub(crate) struct BoothPanes(pub(crate) HashMap<String, f32>);

/// Both directions of the booth-UI bridge: the bake a region samples and the pane geometry the
/// extract publishes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BoothBridge<'w> {
    pub(crate) images: Res<'w, PortraitImages>,
    pub(crate) panes: ResMut<'w, BoothPanes>,
    /// The file panes' half: a tile request per pane, sampled back from the atlas.
    pub(crate) tiles: ResMut<'w, crate::ui_models::UiModelTiles>,
}

/// [`sync_portraits`]'s group inputs and overflow (it sits at Bevy's 16-parameter limit). The name
/// cache is the only race and sex for a member we hold no object for.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PartyBooths<'w, 's> {
    roster: Res<'w, crate::ui_party::GroupState>,
    index: Res<'w, crate::net::GuidIndex>,
    palettes: ResMut<'w, benilla_world::rig_palette::RigPalettes>,
    pet_bar: Res<'w, crate::ui_pet::PetBar>,
    names: Res<'w, crate::names::NameCache>,
    /// What the UI drew last frame, read by `"targettarget"` alone.
    panes: Res<'w, BoothPanes>,
    /// The guid-keyed bake cache, and a fresh target plus the camera's [`RenderTarget`] for the
    /// handover.
    bakes: ResMut<'w, PortraitBakes>,
    images: ResMut<'w, Assets<Image>>,
    targets: Query<'w, 's, (&'static BoothCam, &'static mut RenderTarget)>,
}

/// Tags a booth camera with its slot token (not the M2 rig, `benilla_assets::PortraitCamera`).
#[derive(Component)]
pub(crate) struct BoothCam(pub(crate) String);

/// The display aspect the client's `screencoord` scale uses: `gxResolution`'s width/height with
/// `widescreen` on (its default), `4/3` off. Read by [`framing::pane_model_scale`].
///
/// Deviation: this reads the window's aspect, not `gxResolution` (here only the windowed size)
/// under `widescreen` (not registered), because fullscreen is the monitor's own size with no mode
/// list, where the two agree. Defaults to `4/3`, where the factor is 1.0.
#[derive(Resource)]
pub(crate) struct GxAspect(pub(crate) f32);

impl Default for GxAspect {
    fn default() -> Self {
        Self(4.0 / 3.0)
    }
}

/// Track the primary window's aspect into [`GxAspect`].
fn feed_gx_aspect(
    mut gx: ResMut<GxAspect>,
    window: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let Ok(w) = window.single() else {
        return;
    };
    let (px_w, px_h) = (w.resolution.width(), w.resolution.height());
    if px_h > 0.0 {
        let a = px_w / px_h;
        if (a - gx.0).abs() > 1e-4 {
            gx.0 = a;
        }
    }
}

/// The body panes' render rate: with `boothHalfRate` on (the default), a pane's camera renders
/// every other frame while nothing else keeps it awake; the pose and item effects run at full
/// rate ([`benilla_world::particles::ViewThrottled`]). The glue screens are exempt.
///
/// Deviation: `boothHalfRate` is benilla's own CVar, because the reference draws its doll in the
/// main pass and has no second view; a full-rate pass costs up to 7.6 ms a frame at 4K.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PaneRate {
    /// Welded to the registered CVar's default in `cvars::tests`.
    pub(crate) half: bool,
}

impl Default for PaneRate {
    /// Half-rate, hand-written so the default is not silently `false`.
    fn default() -> Self {
        Self { half: true }
    }
}

/// A body booth's framing inputs: the pane geometry and the display aspect.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BoothFraming<'w> {
    pub(crate) panes: Res<'w, BoothPanes>,
    pub(crate) gx: Res<'w, GxAspect>,
}

/// Owns the portrait bake pipeline: the [`PortraitImages`] bridge and the per-slot booths.
pub(crate) struct PortraitPlugin;

/// `boothHalfRate`'s change callback.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut rate: ResMut<PaneRate>) {
    if ev.is("boothHalfRate") {
        rate.half = ev.flag();
    }
}

impl Plugin for PortraitPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PortraitImages>()
            .init_resource::<PortraitBakes>()
            .init_resource::<PaneRate>()
            .add_observer(on_cvar)
            .init_resource::<PaperDollBooth>()
            .init_resource::<InspectBooth>()
            .init_resource::<PetDollBooth>()
            .init_resource::<StableBooth>()
            .init_resource::<StableStandIn>()
            .init_resource::<glue_booth::GluePreview>()
            .init_resource::<glue_booth::GluePreviewBake>()
            .init_resource::<glue_booth::GluePetBake>()
            .init_resource::<dressup::DressUpPreview>()
            .init_resource::<dressup::DressUpBake>()
            .init_resource::<Booths>()
            .init_resource::<BoothPanes>()
            .init_resource::<GxAspect>()
            .init_resource::<BoothLight>()
            .add_systems(Startup, setup_booths)
            // The variant-cache reaper: booth twins die with their world source material.
            .add_systems(Update, (light::reap_dead_variants, feed_gx_aspect))
            // The test bake owns the booths when its env is set; the chain orders the shared
            // camera, booth and image access.
            .add_systems(
                Update,
                (
                    // First, so every pane reads one revision. A bump lands next frame, the
                    // reference's own latency (an event queued into `0x524cd0`'s per-frame drain).
                    bump_model_revision,
                    test_bake::sync_test_portraits,
                    sync_portraits,
                    sync_paperdoll,
                    sync_inspect_booth,
                    sync_petdoll_booth,
                    // The stand-in must exist before the booth mirrors it.
                    sync_stable_standin,
                    sync_stable_booth,
                    // The scene before the character standing in it: they commit as one pair
                    // (`glue_booth::PendingSwap`), and the booth reads the stage off the scene.
                    glue_booth::sync_glue_scene,
                    glue_booth::sync_glue_booth,
                    // The glue screens' FFX state, off the same look every screen already writes.
                    glue_booth::sync_glue_ffx,
                    // After the scene's framing: the viewport and clear it decided.
                    glue_booth::pillarbox_glue_scene,
                    // After the scene: the pet's seat and its light are the scene's to publish.
                    glue_booth::sync_glue_pet,
                    dressup::sync_dressup_booth,
                    // After the syncs, the only writers of the yaw it spends.
                    booth::drive_booth_turn,
                    // Last: it reads the wake/pending state every sync above may have armed.
                    gate_booth_cameras,
                )
                    .chain(),
            )
            // Re-face each booth's eye-glow cards to its own camera.
            .add_systems(Update, booth::face_booth_billboards)
            // Push each booth part's sampled `MatAnim` alpha, after the sampler.
            .add_systems(
                Update,
                booth::push_booth_mat_alpha.after(benilla_world::doodad_anim::sample_mat_anim),
            )
            // `WOW_CREATE_TEST`: the create-screen preview instrument.
            .add_systems(Update, glue_booth::drive_create_test)
            // `WOW_BOOTH_DUMP=<token>:<path>:<secs>`: write a booth's target to disk.
            .add_systems(Update, test_bake::dump_booth_target)
            // `WOW_BOOTH_LOG=1`: the model half of the framing instrument.
            .add_systems(Update, log_booth_pose);
    }
}

/// A fresh transparent render-target image of `size²`: a camera target the UI samples.
fn new_target_image(size: u32) -> Image {
    new_target_image_sized(size, size)
}

/// [`new_target_image`] at any size, for the UI model tiles' atlas (`crate::ui_models`).
pub(crate) fn new_target_image_sized(width: u32, height: u32) -> Image {
    let mut image = Image::new_fill(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0; 8],
        // Un-encoded: the UI composites in gamma bytes and decodes once at the end (`ui_gamma`),
        // so an `…Srgb` target would decode twice. Float: 8-bit un-encoded values band visibly.
        TextureFormat::Rgba16Float,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    image
}

/// The booth view shape, which sets a booth camera's pipeline key space. Every booth camera
/// spawns it, because the warm pass compiles against exactly this shape.
pub(crate) fn booth_view_shape() -> impl Bundle {
    (
        Camera3d::default(),
        bevy::render::view::Hdr,
        Tonemapping::None,
        Msaa::Off,
        // No Bevy light reaches a booth, so skip the per-view cluster rebuild.
        bevy::light::cluster::ClusterConfig::None,
    )
}

/// The warm pass's twin booth: the booth view shape with the custom projection class real bakes
/// install, a pipeline key space of its own. Real booths are never stamped for warming.
pub(crate) fn spawn_warm_booth(
    commands: &mut Commands,
    images: &mut Assets<Image>,
) -> (Entity, RenderLayers) {
    let layer = RenderLayers::layer(WARM_BOOTH_LAYER);
    let image = images.add(new_target_image(PORTRAIT_SIZE));
    let cam = commands
        .spawn((
            booth_view_shape(),
            Camera {
                order: -100 + WARM_BOOTH_LAYER as isize,
                clear_color: ClearColorConfig::Custom(Color::srgb(0.055, 0.045, 0.04)),
                ..default()
            },
            RenderTarget::Image(image.into()),
            benilla_world::ffx_glow::FfxGlow::BOOTH,
            Projection::custom(framing::WowPortraitProjection {
                fov: framing::PANE_FIXED_FOV,
                near: 0.02,
                far: 100.0,
                // The warm pass compiles pipelines, which the aspect does not key.
                aspect: 1.0,
            }),
            layer.clone(),
        ))
        .id();
    (cam, layer)
}

/// Startup: one booth per slot, with its image, a model root, and a camera rendering that slot's
/// layer before the world camera.
fn setup_booths(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut portraits: ResMut<PortraitImages>,
    mut booths: ResMut<Booths>,
    mut booth_light: ResMut<BoothLight>,
    mut mirrors: ResMut<benilla_world::rig_palette::RigPaletteMirrors>,
    device: Res<bevy::render::renderer::RenderDevice>,
    queue: Res<bevy::render::renderer::RenderQueue>,
) {
    // The studio-light buffer, written once. Sized to the full blob, which wgpu validates against
    // the shader's struct; the zeroed rest leaves `point_count = 0`.
    let light_buffer = |label: &'static str, blob: benilla_world::lighting::LightBlob| {
        let buffer = blob.create(&device, label);
        blob.write(&queue, &buffer);
        buffer
    };
    booth_light.studio.buffer = Some(light_buffer("wow_portrait_light", studio_light()));
    // The body panes' own light, the reference `<PlayerModel>` widget's.
    booth_light.pane.buffer = Some(light_buffer("wow_model_pane_light", model_pane_light()));
    // Booth rigs skin from the palette regions of these buffers: register both
    // as mirrors so the palette upload keeps their regions live.
    for (key, buf) in [
        ("portrait", &booth_light.studio.buffer),
        ("pane", &booth_light.pane.buffer),
    ] {
        if let Some(b) = buf {
            mirrors.0.insert(key, b.clone());
        }
    }

    for (i, token) in SLOTS.iter().enumerate() {
        let image = images.add(new_target_image(PORTRAIT_SIZE));
        portraits
            .0
            .insert((*token).to_string(), PortraitSource::Live(image.clone()));
        let layer = RenderLayers::layer(PORTRAIT_LAYER_BASE + i);
        let root = commands
            .spawn((Transform::IDENTITY, Visibility::Visible, layer.clone()))
            .id();
        commands.spawn((
            booth_view_shape(),
            Camera {
                // Booths render first (negative order), one order per slot.
                order: -100 + i as isize,
                // Deviation: the reference widget draws only its model; a body pane clears to
                // near-black, the chosen look where nothing sits behind the pane.
                clear_color: ClearColorConfig::Custom(Color::srgb(0.055, 0.045, 0.04)),
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            // The FFXGlow combine owns the one gamma decode, at world parity.
            benilla_world::ffx_glow::FfxGlow::BOOTH,
            Projection::from(PerspectiveProjection {
                fov: PORTRAIT_FOV,
                near: 0.02,
                far: 100.0,
                ..default()
            }),
            layer.clone(),
            BoothCam((*token).to_string()),
        ));
        booths.0.insert(
            (*token).to_string(),
            Booth {
                layer,
                root,
                target: image,
                baked: None,
                baked_guid: None,
                snap: None,
                shown: false,
                show_rev: 0,
                wake: 0,
                live: false,
                pending: Vec::new(),
                pending_since: None,
                pipes_settling: false,
                pipes_since: None,
                aspect: 1.0,
                rigged: false,
                parked: false,
                turn: Turn::default(),
            },
        );
    }

    // The four body booths: the paper doll, inspect, the pet paper doll and the stable
    // (`0x4cb870`), at 512², aimed per bake by `sync_body_booth`.
    for (i, (slot, layer_index)) in [
        (PAPERDOLL_SLOT, PAPERDOLL_LAYER),
        (INSPECT_SLOT, INSPECT_LAYER),
        (PETDOLL_SLOT, PETDOLL_LAYER),
        (STABLE_SLOT, STABLE_LAYER),
    ]
    .into_iter()
    .enumerate()
    {
        let image = images.add(new_target_image(PAPERDOLL_SIZE));
        portraits
            .0
            .insert(slot.to_string(), PortraitSource::Live(image.clone()));
        let layer = RenderLayers::layer(layer_index);
        let root = commands
            .spawn((Transform::IDENTITY, Visibility::Visible, layer.clone()))
            .id();
        commands.spawn((
            booth_view_shape(),
            Camera {
                order: -100 + (SLOTS.len() + i) as isize,
                // Transparent: a `<PlayerModel>` has no backdrop; the page's art shows behind.
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            // No glow: the reference paints `<PlayerModel>` widgets after the WorldFrame's FFX.
            benilla_world::ffx_glow::FfxGlow::UI_PANE,
            // A placeholder until the first bake.
            Projection::from(PerspectiveProjection {
                fov: PORTRAIT_FOV,
                near: 0.02,
                far: 100.0,
                ..default()
            }),
            layer.clone(),
            BoothCam(slot.to_string()),
        ));
        booths.0.insert(
            slot.to_string(),
            Booth {
                layer,
                root,
                target: image,
                baked: None,
                baked_guid: None,
                snap: None,
                shown: false,
                show_rev: 0,
                wake: 0,
                live: false,
                pending: Vec::new(),
                pending_since: None,
                pipes_settling: false,
                pipes_since: None,
                aspect: 1.0,
                rigged: false,
                parked: false,
                turn: Turn::default(),
            },
        );
    }

    // The glue booth: its own slot/layer/target, framed per-bake.
    glue_booth::spawn_glue_booth(&mut commands, &mut images, &mut portraits, &mut booths);
    // The dressing room: tuple-driven like the glue booth, lit like the paper doll.
    dressup::spawn_dressup_booth(&mut commands, &mut images, &mut portraits, &mut booths);
}

/// `true` while the `WOW_PORTRAIT_TEST` debug bake owns the booths (read once).
fn test_mode(cached: &mut Option<bool>) -> bool {
    *cached.get_or_insert_with(|| std::env::var("WOW_PORTRAIT_TEST").is_ok_and(|s| !s.is_empty()))
}

/// benilla's `UNIT_MODEL_CHANGED` for the producers that are not a change of dress; a body pane
/// re-takes its snapshot when it moves ([`SnapKey`]).
///
/// - The manual sheath ceremony: `ToggleSheath` passes `bInstant = 0`, and the clip's
///   `$SHL`/`$SHR` event marks the unit (`0x611b60` → `0x5ffb10`, mark at `0x5ffbbe`). A snap
///   sheath change (combat auto-draw) reaches the queue only through the enchant-gated `0x5eed50`.
/// - An item glow landing: not the reference's, whose widget duplicates a finished model.
#[derive(Component, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ModelRevision(pub(crate) u32);

/// Bump [`ModelRevision`] for every unit one of its producers fired on this frame.
fn bump_model_revision(
    mut commands: Commands,
    mut swaps: MessageReader<crate::creature_anim::SheathSwapMessage>,
    glows: Query<&benilla_world::model_fade::ParentModel, Added<crate::entities::ItemGlowAttached>>,
    revs: Query<&ModelRevision>,
) {
    let mut bumped = <bevy::platform::collections::HashSet<Entity>>::default();
    let units = swaps
        .read()
        .map(|m| m.entity)
        // A glow's root is chained to its wearer, the unit that gained geometry.
        .chain(glows.iter().map(|p| p.0));
    for unit in units {
        if !bumped.insert(unit) {
            continue;
        }
        let next = revs.get(unit).map_or(0, |r| r.0).wrapping_add(1);
        commands.entity(unit).try_insert(ModelRevision(next));
    }
}

/// The attach ids the sheath lane owns: the shield's forearm, the hands, the sheath points and the
/// shield's back slot. A stowed ranged weapon (`0x611770`), a melee weapon with `SheatheType` 0 and
/// the worn quiver (`0x1a`) render nothing; [`SnapKey`] ignores the whole lane.
fn sheath_lane(attach: Option<u16>) -> bool {
    matches!(attach, Some(0..=2 | 26..=28 | 30..=33))
}

/// What a body pane re-takes its snapshot on. A `<PlayerModel>` duplicates the unit's `CM2Model`
/// once (`0x5059a0` → `0x707400`, which releases the source) and re-takes it on four things:
///
/// 1. The pane becoming visible ([`Self::show`]): the show override `0x505d00` re-duplicates on
///    every hidden-to-visible edge; `0x505ce0` destroys on hide.
/// 2. A change of dress ([`Self::dress`], [`Self::stable`]): `UNIT_MODEL_CHANGED`, fired only at
///    `0x524df1`, marked by equipping (`0x5e2810` → `0x5dee30`), a displayId change, a helm or
///    cloak toggle and an enchant; not by nocked ammo (`0x60ba30`) or combat.
/// 3. A model event ([`Self::rev`], [`ModelRevision`]).
/// 4. A resize ([`Self::aspect_bits`]): `DISPLAY_SIZE_CHANGED` → `RefreshUnit()`.
#[derive(PartialEq, Eq)]
struct SnapKey {
    /// A different body is a different `SetUnit`.
    unit: Entity,
    /// The mirrored geometry outside [`sheath_lane`]. The body's armour composite is shared by
    /// pointer in the reference, so a re-blit reaches an open doll.
    stable: Vec<(AssetId<Mesh>, AssetId<WowModelMaterial>)>,
    stable_fx: Vec<(u16, [u32; 3], usize)>,
    /// The weapon slots, resolved above the placement gate: sheath state can decide whether an
    /// item exists at all.
    dress: Option<crate::entities::DressKey>,
    rev: u32,
    show: u32,
    aspect_bits: u32,
}

impl SnapKey {
    /// The key for `unit`'s pane this frame.
    fn build(
        unit: Entity,
        parts: &[&PortraitPart],
        riders: &[&PortraitRider],
        billboards: &[&PortraitBillboard],
        effects: &[&PortraitEffects],
        dress: Option<crate::entities::DressKey>,
        rev: u32,
        show: u32,
        aspect: f32,
    ) -> Self {
        // Sorted, so sibling order cannot move the key.
        let mut stable: Vec<(AssetId<Mesh>, AssetId<WowModelMaterial>)> = parts
            .iter()
            .map(|p| (p.static_mesh.id(), p.material.id()))
            .chain(
                riders
                    .iter()
                    .filter(|r| !sheath_lane(r.attach))
                    .map(|r| (r.static_mesh.id(), r.material.id())),
            )
            .chain(
                billboards
                    .iter()
                    .filter(|b| !sheath_lane(b.attach))
                    .map(|b| (b.mesh.id(), b.material.id())),
            )
            .collect();
        stable.sort_unstable();
        let mut stable_fx: Vec<(u16, [u32; 3], usize)> = effects
            .iter()
            .filter(|f| !sheath_lane(f.attach))
            .map(|e| {
                (
                    e.bone,
                    e.offset.to_array().map(f32::to_bits),
                    e.emitters.len(),
                )
            })
            .collect();
        stable_fx.sort_unstable();
        SnapKey {
            unit,
            stable,
            stable_fx,
            dress,
            rev,
            show,
            aspect_bits: aspect.to_bits(),
        }
    }
}

/// The reference's attach reset: does `0x47a230` detach a sub-model at this attachment id from a
/// widget's duplicate (`0x707400`/`0x70ea00`; the paper doll via `0x5059a0`, the round portrait
/// via `0x525261`)? It cuts `0xf`–`0x19`, `0x1d`, `0x22` and `0x23` (the nocked arrow) and keeps
/// the hands, the sheath family and the worn slots.
fn attach_reset(attach: Option<u16>) -> bool {
    matches!(attach, Some(0xf..=0x19 | 0x1d | 0x22 | 0x23))
}

/// The reference's hand grip for a widget's duplicate, `[right, left]`, from attachment occupancy
/// after the reset (`0x5059a0`: attachment 2 closes hand 1, attachment 1 closes hand 0).
fn hand_grip(
    riders: &[&PortraitRider],
    billboards: &[&PortraitBillboard],
    effects: &[&PortraitEffects],
) -> [bool; 2] {
    // A wand can be a camera-facing batch and emitters alone, so probe every lane.
    let occupied = |id: u16| {
        riders.iter().any(|r| r.attach == Some(id))
            || billboards.iter().any(|b| b.attach == Some(id))
            || effects.iter().any(|f| f.attach == Some(id))
    };
    [
        occupied(crate::entities::attach_id::HAND_RIGHT),
        occupied(crate::entities::attach_id::HAND_LEFT),
    ]
}

/// The queries that read a unit's dressed look, sharing one descendants walk.
#[derive(SystemParam)]
struct DressedLook<'w, 's> {
    children: Query<'w, 's, &'static Children>,
    parts: Query<'w, 's, &'static PortraitPart>,
    riders: Query<'w, 's, &'static PortraitRider>,
    billboards: Query<'w, 's, &'static PortraitBillboard>,
    effects: Query<'w, 's, &'static PortraitEffects>,
    mounts: Query<'w, 's, (), With<crate::entities::mount::MountBody>>,
    /// The re-snapshot inputs not on the mirrored tree ([`SnapKey`]).
    dress: Query<'w, 's, &'static crate::entities::DressKey>,
    revs: Query<'w, 's, &'static ModelRevision>,
    stand_in: Query<'w, 's, &'static PortraitStandIn>,
}

impl DressedLook<'_, '_> {
    /// Walk `unit`'s descendants once; empty while the model loads. A mount's parts and `Body`
    /// cards are pruned (`Model:SetUnit` binds the player), but riders, item cards and effects
    /// are kept: mounted, the rider's joints re-root inside the mount subtree.
    fn collect(
        &self,
        unit: Entity,
    ) -> (
        Vec<&PortraitPart>,
        Vec<&PortraitRider>,
        Vec<&PortraitBillboard>,
        Vec<&PortraitEffects>,
    ) {
        let mut parts = Vec::new();
        let mut riders = Vec::new();
        let mut billboards = Vec::new();
        let mut effects = Vec::new();
        let mut stack: Vec<(Entity, bool)> = vec![(unit, false)];
        while let Some((e, mut in_mount)) = stack.pop() {
            in_mount |= self.mounts.contains(e);
            if !in_mount {
                if let Ok(p) = self.parts.get(e) {
                    parts.push(p);
                }
            }
            if let Ok(r) = self.riders.get(e) {
                if !attach_reset(r.attach) {
                    riders.push(r);
                }
            }
            if let Ok(b) = self.billboards.get(e) {
                if (!in_mount || b.seat != PortraitSeat::Body) && !attach_reset(b.attach) {
                    billboards.push(b);
                }
            }
            // Never pruned: every publisher is an item's model.
            if let Ok(fx) = self.effects.get(e) {
                if !attach_reset(fx.attach) {
                    effects.push(fx);
                }
            }
            if let Ok(c) = self.children.get(e) {
                stack.extend(c.iter().map(|child| (child, in_mount)));
            }
        }
        (parts, riders, billboards, effects)
    }

    fn stand_in_display(&self, unit: Entity) -> Option<u32> {
        self.stand_in.get(unit).ok().map(|s| s.0)
    }

    fn snapshot_inputs(&self, unit: Entity) -> (Option<crate::entities::DressKey>, u32) {
        (
            self.dress.get(unit).ok().copied(),
            self.revs.get(unit).map_or(0, |r| r.0),
        )
    }
}

/// Each frame, mirror each slot's unit's dressed look into its booth whenever it changes; a unit
/// whose model has not attached shows a 2D stand-in, where the reference shows blank (`0x519fcc`).
fn sync_portraits(
    mut commands: Commands,
    mut booths: ResMut<Booths>,
    mut portraits: ResMut<PortraitImages>,
    mut booth_light: ResMut<BoothLight>,
    creatures: Option<Res<Creatures>>,
    selection: Res<Selection>,
    self_q: Query<Entity, With<SelfPlayer>>,
    ent_q: Query<&NetEntity>,
    stores_q: Query<&crate::net::ObjectStore>,
    look: DressedLook,
    mut wow_mats: ResMut<Assets<WowModelMaterial>>,
    mut env_cache: Local<Option<bool>>,
    mut cams: Query<(&BoothCam, &mut Transform, &mut Projection)>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    interact_npc: Res<crate::ui_session::InteractNpc>,
    mut party: PartyBooths,
) {
    if test_mode(&mut env_cache) {
        return; // the test bake owns the booths
    }
    for token in SLOTS {
        // A unit the object manager does not hold draws the 2D stand-in (`0x525ba0`), not the
        // blank, which is for a model not ready (`0x519fcc`); `unseen` carries that art.
        let mut unseen: Option<String> = None;
        let mut occupant: Option<u64> = None;
        let unit: Option<Entity> = match token {
            "player" => self_q.single().ok(),
            "target" => selection.target,
            // Gated on the UI drawing the slot: its frame ships hidden (`SHOW_TARGET_OF_TARGET`
            // defaults to `"0"`), and ungated it would re-bake on every target switch.
            "targettarget" => party
                .panes
                .0
                .contains_key("targettarget")
                .then(|| {
                    selection
                        .target
                        .and_then(|e| stores_q.get(e).ok())
                        .and_then(|s| s.0.unit_target())
                        .filter(|g| *g != 0)
                        .and_then(|g| party.index.0.get(&g).copied())
                })
                .flatten(),
            "pet" => {
                let guid = party.pet_bar.spells.pet_guid;
                let entity = (guid != 0)
                    .then(|| party.index.0.get(&guid))
                    .flatten()
                    .copied();
                if entity.is_none() && guid != 0 {
                    unseen = Some(creature_temporary_portrait());
                }
                entity
            }
            "npc" => interact_npc.0,
            // Out of range, the stand-in's race and sex come from the name cache.
            tok => {
                let member = tok
                    .strip_prefix("party")
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|n| party.roster.party_slots().nth(n - 1));
                // The key the handover files under and the cache probe uses.
                occupant = member.map(|m| m.guid);
                let entity = member.and_then(|m| party.index.0.get(&m.guid)).copied();
                if entity.is_none() {
                    unseen = member.map(|m| {
                        let traits = party.names.player_traits(m.guid);
                        player_temporary_portrait(
                            traits.map(|(race, _, _)| race),
                            traits.map(|(_, _, sex)| sex),
                        )
                    });
                }
                entity
            }
        };
        let Some(booth) = booths.0.get_mut(token) else {
            continue;
        };
        let Some(unit) = unit else {
            // The bake handover: the target holds the standing face, so it moves into the guid
            // cache and the booth gets a fresh one; the UI keeps the same handle, the reference's
            // "bind the cached bake" (`0x525c36`). Only a settled bake: a half-drawn one would
            // freeze.
            if let Some(guid) = booth.baked_guid.filter(|_| {
                booth.baked.is_some()
                    && booth.wake == 0
                    && booth.pending.is_empty()
                    && !booth.pipes_settling
            }) {
                let fresh = party.images.add(new_target_image(PORTRAIT_SIZE));
                for (cam, mut target) in &mut party.targets {
                    if cam.0 == *token {
                        *target = RenderTarget::Image(fresh.clone().into());
                    }
                }
                party
                    .bakes
                    .store(guid, std::mem::replace(&mut booth.target, fresh));
            }
            booth.baked_guid = None;
            // No unit: empty the booth.
            if booth.baked.is_some() {
                commands.entity(booth.root).despawn_related::<Children>();
                booth.baked = None;
                // Render the emptied stage, so the target holds the backdrop.
                booth.wake = BOOTH_SETTLE_FRAMES;
                booth.pending.clear();
                // The root's rig state needs its own strip.
                clear_booth_rig(&mut commands, booth.root);
                booth.rigged = false;
                booth.parked = false;
            }
            // No object: the cached bake on a hit, the 2D stand-in on a miss (`0x525ba0`).
            let src = match unseen {
                Some(file) => occupant
                    .and_then(|g| party.bakes.get(g))
                    .map_or(PortraitSource::File(file), PortraitSource::Live),
                None => PortraitSource::Live(booth.target.clone()),
            };
            if portraits.0.get(token) != Some(&src) {
                portraits.0.insert(token.to_string(), src);
            }
            continue;
        };
        let (parts, riders, billboards, effects) = look.collect(unit);
        if parts.is_empty() {
            // Not attached yet: a 2D stand-in. The reference blanks the widget (`0x519fcc`) and
            // re-bakes once the model is ready.
            let file = temporary_portrait(ent_q.get(unit).ok(), stores_q.get(unit).ok());
            let src = PortraitSource::File(file);
            if portraits.0.get(token) != Some(&src) {
                portraits.0.insert(token.to_string(), src);
            }
            continue;
        }
        let key = LookKey::build(&parts, &riders, &billboards, &effects);
        // A changed occupant re-bakes even at an identical look, or the handover files one
        // member's face under another's guid.
        if booth.baked.as_ref() != Some(&key) || booth.baked_guid != occupant {
            let display_id = ent_q.get(unit).ok().and_then(|n| n.display_id);
            // Resolve the anchors before any teardown; a still-loading display waits.
            let Some(anchors) = booth_anchors(creatures.as_deref(), display_id) else {
                booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
                log_bake(
                    token,
                    "wait-anchors",
                    &parts,
                    &riders,
                    &billboards,
                    &effects,
                );
                continue;
            };
            let rig = creatures
                .as_deref()
                .zip(display_id)
                .and_then(|(c, d)| c.display_rig(d));
            let booth_parts: Vec<BoothPart> = parts
                .iter()
                .map(|p| BoothPart {
                    skinned: p.skinned_mesh.clone(),
                    static_mesh: p.static_mesh.clone(),
                    material: booth_light.studio.variant(&p.material, &mut wow_mats),
                    // `None`: a mirrored part does not carry the batch's alpha loops.
                    alpha_anim: None,
                    twins: BoothTwins::default(),
                    mat_anim: false,
                })
                .collect();
            let booth_riders: Vec<BoothRider> = riders
                .iter()
                .map(|r| BoothRider {
                    mesh: r.static_mesh.clone(),
                    material: booth_light.studio.variant(&r.material, &mut wow_mats),
                    bone: r.bone,
                    offset: r.offset,
                    twins: BoothTwins::default(),
                })
                .collect();
            let booth_billboards: Vec<BoothBillboardSpec> = billboards
                .iter()
                .map(|b| BoothBillboardSpec {
                    mesh: b.mesh.clone(),
                    material: booth_light.studio.variant(&b.material, &mut wow_mats),
                    bone: b.bone,
                    offset: b.seat.offset(),
                    kind: b.kind,
                    twins: BoothTwins::default(),
                })
                .collect();
            // A source material was not resident, so a twin is the world one; retry next frame.
            if booth_light.studio.take_unready() {
                booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
                continue;
            }
            commands.entity(booth.root).despawn_related::<Children>();
            let booth_rig = spawn_booth_model(
                &mut commands,
                &mut party.palettes,
                booth.root,
                booth.layer.clone(),
                &booth_parts,
                &booth_riders,
                rig.as_ref().and_then(|r| {
                    r.inverse_bindposes
                        .as_ref()
                        .map(|ibp| (r.skeleton, ibp, r.animations))
                }),
                anim_data.as_deref().map(|a| &a.0),
                BoothMotion::Frozen,
                // Open hands: the reference's portrait bake (`0x524f60`) runs the attach reset but,
                // unlike `0x5059a0`, never probes the hand points or calls `0x479660`.
                [false, false],
                &booth_billboards,
                BoothInstance::default(),
            );
            // A frozen still re-evaluates its pose each frame, so the park matters most here.
            booth.rigged = booth_rig.rigged();
            booth_rig.finish(&mut commands);
            booth.parked = false;
            // No emitters: the round portrait is a one-shot bake (`0x524f60`, one `0x707680` draw),
            // and a fresh particle pool contributes nothing to one frame.
            log_frame(token, &anchors, &frame(&anchors).0);
            aim(&mut cams, token, &frame(&anchors));
            log_bake(token, "bake", &parts, &riders, &billboards, &effects);
            wake_booth(
                booth,
                &wow_mats,
                booth_parts
                    .iter()
                    .map(|p| &p.material)
                    .chain(booth_riders.iter().map(|r| &r.material))
                    .chain(booth_billboards.iter().map(|b| &b.material)),
            );
            booth.baked = Some(key);
            booth.baked_guid = occupant;
        }
        let live = PortraitSource::Live(booth.target.clone());
        if portraits.0.get(token) != Some(&live) {
            portraits.0.insert(token.to_string(), live);
        }
    }
}

/// Each frame, bake the self player's dressed look into the paper-doll booth, full-body and spun
/// to [`PaperDollBooth::yaw`]. A [`SnapKey`] change re-bakes; a yaw change only re-rotates.
fn sync_paperdoll(
    mut commands: Commands,
    mut booths: ResMut<Booths>,
    mut portraits: ResMut<PortraitImages>,
    mut booth_light: ResMut<BoothLight>,
    creatures: Option<Res<Creatures>>,
    self_q: Query<Entity, With<SelfPlayer>>,
    ent_q: Query<&NetEntity>,
    look: DressedLook,
    paperdoll: Res<PaperDollBooth>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut wow_mats: ResMut<Assets<WowModelMaterial>>,
    mut env_cache: Local<Option<bool>>,
    mut last_pose: Local<Option<(f32, f32)>>,
    mut cams: Query<(&BoothCam, &mut Transform, &mut Projection)>,
    framing_in: BoothFraming,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
) {
    if test_mode(&mut env_cache) {
        return; // the test bake owns the booths
    }
    sync_body_booth(
        &mut palettes,
        PAPERDOLL_SLOT,
        self_q.single().ok(),
        paperdoll.yaw,
        &mut last_pose,
        framing_in.gx.0,
        &mut commands,
        &mut booths,
        &mut portraits,
        &mut booth_light,
        creatures.as_deref(),
        &ent_q,
        &look,
        &mut wow_mats,
        &mut cams,
        anim_data.as_deref(),
        &framing_in.panes,
    );
}

/// The inspect window's pane, pointed at [`crate::ui_inspect`]'s unit.
fn sync_inspect_booth(
    mut commands: Commands,
    mut booths: ResMut<Booths>,
    mut portraits: ResMut<PortraitImages>,
    mut booth_light: ResMut<BoothLight>,
    creatures: Option<Res<Creatures>>,
    ent_q: Query<&NetEntity>,
    look: DressedLook,
    inspect: Res<InspectBooth>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut wow_mats: ResMut<Assets<WowModelMaterial>>,
    mut env_cache: Local<Option<bool>>,
    mut last_pose: Local<Option<(f32, f32)>>,
    mut cams: Query<(&BoothCam, &mut Transform, &mut Projection)>,
    framing_in: BoothFraming,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
) {
    if test_mode(&mut env_cache) {
        return;
    }
    sync_body_booth(
        &mut palettes,
        INSPECT_SLOT,
        inspect.unit,
        inspect.yaw,
        &mut last_pose,
        framing_in.gx.0,
        &mut commands,
        &mut booths,
        &mut portraits,
        &mut booth_light,
        creatures.as_deref(),
        &ent_q,
        &look,
        &mut wow_mats,
        &mut cams,
        anim_data.as_deref(),
        &framing_in.panes,
    );
}

/// The pet paper doll's pane, pointed at [`crate::ui_pet_doll`]'s pet.
fn sync_petdoll_booth(
    mut commands: Commands,
    mut booths: ResMut<Booths>,
    mut portraits: ResMut<PortraitImages>,
    mut booth_light: ResMut<BoothLight>,
    creatures: Option<Res<Creatures>>,
    ent_q: Query<&NetEntity>,
    look: DressedLook,
    petdoll: Res<PetDollBooth>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut wow_mats: ResMut<Assets<WowModelMaterial>>,
    mut env_cache: Local<Option<bool>>,
    mut last_pose: Local<Option<(f32, f32)>>,
    mut cams: Query<(&BoothCam, &mut Transform, &mut Projection)>,
    framing_in: BoothFraming,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
) {
    if test_mode(&mut env_cache) {
        return;
    }
    sync_body_booth(
        &mut palettes,
        PETDOLL_SLOT,
        petdoll.unit,
        petdoll.yaw,
        &mut last_pose,
        framing_in.gx.0,
        &mut commands,
        &mut booths,
        &mut portraits,
        &mut booth_light,
        creatures.as_deref(),
        &ent_q,
        &look,
        &mut wow_mats,
        &mut cams,
        anim_data.as_deref(),
        &framing_in.panes,
    );
}

/// The stable pane's stand-in subject ([`PortraitStandIn`]), keyed on the display: two pets that
/// share one are the same model, as in the reference's cached branch.
#[derive(Resource, Default)]
struct StableStandIn {
    /// The stand-in, while one exists.
    entity: Option<Entity>,
    /// The display it was spawned for.
    display: Option<u32>,
    /// Its mirror children are up.
    built: bool,
}

/// Keep [`StableStandIn`] on the selected stabled pet, rebuilding it when the want changes.
fn sync_stable_standin(
    mut commands: Commands,
    stable: Res<StableBooth>,
    creatures: Option<Res<Creatures>>,
    mut state: ResMut<StableStandIn>,
) {
    // The live pet wins: the reference reaches the creature cache only after the GUID
    // resolve fails (`0x4cb9bc`).
    let want = stable.unit.is_none().then_some(stable.display_id).flatten();
    if state.display != want {
        if let Some(old) = state.entity.take() {
            commands.entity(old).despawn();
        }
        state.display = want;
        state.built = false;
        state.entity = want.map(|display| {
            commands
                .spawn((
                    PortraitStandIn(display),
                    // Hidden: the booth draws its own copies.
                    Transform::default(),
                    Visibility::Hidden,
                ))
                .id()
        });
    }
    if state.built {
        return;
    }
    let (Some(root), Some(disp), Some(creatures)) = (state.entity, want, creatures.as_deref())
    else {
        return;
    };
    // `None` while the display's model loads.
    let Some((parts, cards)) = creatures.display_mirror(disp) else {
        return;
    };
    let (n_parts, n_cards) = (parts.len(), cards.len());
    commands.entity(root).with_children(|kids| {
        for part in parts {
            kids.spawn((Transform::default(), Visibility::Inherited, part));
        }
        for card in cards {
            kids.spawn((Transform::default(), Visibility::Inherited, card));
        }
    });
    state.built = true;
    debug!(
        "stable booth: stand-in for display {disp} — {n_parts} part(s), {n_cards} camera-facing"
    );
}

/// The stable window's pane, with the stand-in as the fallback subject.
fn sync_stable_booth(
    mut commands: Commands,
    mut booths: ResMut<Booths>,
    mut portraits: ResMut<PortraitImages>,
    mut booth_light: ResMut<BoothLight>,
    creatures: Option<Res<Creatures>>,
    ent_q: Query<&NetEntity>,
    look: DressedLook,
    stable: Res<StableBooth>,
    stand_in: Res<StableStandIn>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut wow_mats: ResMut<Assets<WowModelMaterial>>,
    mut env_cache: Local<Option<bool>>,
    mut last_pose: Local<Option<(f32, f32)>>,
    mut cams: Query<(&BoothCam, &mut Transform, &mut Projection)>,
    framing_in: BoothFraming,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
) {
    if test_mode(&mut env_cache) {
        return;
    }
    sync_body_booth(
        &mut palettes,
        STABLE_SLOT,
        stable.unit.or(stand_in.entity),
        stable.yaw,
        &mut last_pose,
        framing_in.gx.0,
        &mut commands,
        &mut booths,
        &mut portraits,
        &mut booth_light,
        creatures.as_deref(),
        &ent_q,
        &look,
        &mut wow_mats,
        &mut cams,
        anim_data.as_deref(),
        &framing_in.panes,
    );
}

/// Bake `unit`'s full-body dressed look into the `slot` booth at `yaw`, for all four body panes;
/// `None` empties it. `unit` may be a [`PortraitStandIn`].
fn sync_body_booth(
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    slot: &str,
    unit: Option<Entity>,
    yaw: f32,
    last_pose: &mut Option<(f32, f32)>,
    display_aspect: f32,
    commands: &mut Commands,
    booths: &mut Booths,
    portraits: &mut PortraitImages,
    booth_light: &mut BoothLight,
    creatures: Option<&Creatures>,
    ent_q: &Query<&NetEntity>,
    look: &DressedLook,
    wow_mats: &mut Assets<WowModelMaterial>,
    cams: &mut Query<(&BoothCam, &mut Transform, &mut Projection)>,
    anim_data: Option<&crate::creature_anim::AnimData>,
    panes: &BoothPanes,
) {
    let Some(booth) = booths.0.get_mut(slot) else {
        return;
    };
    // Latch the pane's aspect while it is drawn.
    let aspect = panes.0.get(slot).copied().unwrap_or(booth.aspect);
    // The show edge (`0x505d00`): `BoothPanes` publishes a slot exactly while its pane is drawn.
    let on_screen = panes.0.contains_key(slot);
    if on_screen && !booth.shown {
        booth.show_rev = booth.show_rev.wrapping_add(1);
    }
    booth.shown = on_screen;
    let live = PortraitSource::Live(booth.target.clone());
    if portraits.0.get(slot) != Some(&live) {
        portraits.0.insert(slot.to_string(), live);
    }
    let (parts, riders, billboards, effects) = match unit {
        Some(unit) => look.collect(unit),
        None => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
    };
    if parts.is_empty() {
        // Empty the booth and forget the applied yaw.
        if booth.snap.is_some() {
            commands.entity(booth.root).despawn_related::<Children>();
            booth.snap = None;
            *last_pose = None;
            // Render the emptied stage; with no emitters left it is not live.
            booth.wake = BOOTH_SETTLE_FRAMES;
            booth.live = false;
            booth.pending.clear();
            clear_booth_rig(commands, booth.root);
            booth.rigged = false;
            booth.parked = false;
        }
        return;
    }
    let unit = unit.expect("unit present — parts came from its descendants");
    let (dress, rev) = look.snapshot_inputs(unit);
    let key = SnapKey::build(
        unit,
        &parts,
        &riders,
        &billboards,
        &effects,
        dress,
        rev,
        booth.show_rev,
        aspect,
    );
    // A stand-in carries its display on itself.
    let display_id = ent_q
        .get(unit)
        .ok()
        .and_then(|n| n.display_id)
        .or_else(|| look.stand_in_display(unit));
    // Anchors before any teardown; the root scale needs them on idle frames too.
    let anchors_now = booth_anchors(creatures, display_id);
    let model_scale = anchors_now
        .as_ref()
        .map_or(1.0, |a| framing::pane_root_scale(a, display_aspect));
    // The reference's re-`SetUnit` set (`SnapKey`), which a weapon draw is not in.
    let parts_changed = booth.snap.as_ref() != Some(&key);
    if parts_changed {
        booth.aspect = aspect;
        let Some(anchors) = anchors_now else {
            booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
            log_bake(slot, "wait-anchors", &parts, &riders, &billboards, &effects);
            return;
        };
        let rig = creatures
            .zip(display_id)
            .and_then(|(c, d)| c.display_rig(d));
        let booth_parts: Vec<BoothPart> = parts
            .iter()
            .map(|p| BoothPart {
                skinned: p.skinned_mesh.clone(),
                static_mesh: p.static_mesh.clone(),
                material: booth_light.pane.variant(&p.material, wow_mats),
                alpha_anim: None,
                twins: BoothTwins::default(),
                mat_anim: false,
            })
            .collect();
        let booth_riders: Vec<BoothRider> = riders
            .iter()
            .map(|r| BoothRider {
                mesh: r.static_mesh.clone(),
                material: booth_light.pane.variant(&r.material, wow_mats),
                bone: r.bone,
                offset: r.offset,
                twins: BoothTwins::default(),
            })
            .collect();
        let booth_billboards: Vec<BoothBillboardSpec> = billboards
            .iter()
            .map(|b| BoothBillboardSpec {
                mesh: b.mesh.clone(),
                material: booth_light.pane.variant(&b.material, wow_mats),
                bone: b.bone,
                offset: b.seat.offset(),
                kind: b.kind,
                twins: BoothTwins::default(),
            })
            .collect();
        // The worn items' effects, spawned after the model hands over the joints.
        let booth_effects: Vec<BoothEffects> = effects
            .iter()
            .map(|fx| BoothEffects {
                bone: fx.bone,
                offset: fx.offset,
                emitters: fx.emitters.clone(),
            })
            .collect();
        // Never latch a world-lane material into the pane.
        if booth_light.pane.take_unready() {
            booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
            return;
        }
        commands.entity(booth.root).despawn_related::<Children>();
        let mut booth_rig = spawn_booth_model(
            commands,
            palettes,
            booth.root,
            booth.layer.clone(),
            &booth_parts,
            &booth_riders,
            rig.as_ref().and_then(|r| {
                r.inverse_bindposes
                    .as_ref()
                    .map(|ibp| (r.skeleton, ibp, r.animations))
            }),
            anim_data.map(|a| &a.0),
            // The pane animates, as the reference's `<PlayerModel>` renders live.
            BoothMotion::Loop,
            hand_grip(&riders, &billboards, &effects),
            &booth_billboards,
            BoothInstance::default(),
        );
        // Item effects: only the body panes, live widgets in the reference, carry them.
        spawn_booth_effects(
            commands,
            &mut booth_rig,
            &booth.layer,
            booth_light.pane.buffer.as_ref(),
            &booth_effects,
            BoothInstance::default(),
        );
        booth.turn.rebaked();
        // Live, emitters or not: rendered every frame its pane is drawn.
        booth.live = true;
        booth.rigged = booth_rig.rigged();
        booth_rig.finish(commands);
        booth.parked = false;
        log_frame(slot, &anchors, &body_frame(&anchors, aspect).0);
        aim(cams, slot, &body_frame(&anchors, aspect));
        log_bake(slot, "bake", &parts, &riders, &billboards, &effects);
        wake_booth(
            booth,
            wow_mats,
            booth_parts
                .iter()
                .map(|p| &p.material)
                .chain(booth_riders.iter().map(|r| &r.material))
                .chain(booth_billboards.iter().map(|b| &b.material)),
        );
        booth.snap = Some(key);
    }
    // The model root: the widget's `T(pos)·R(facing)·S(s)` with `pos` at the origin. `s`
    // (`framing::pane_root_scale`) is latched with the yaw: both move on a resize.
    //
    // The turn keys on the yaw alone, as `0x505bb0` picks the shuffle before storing the angle,
    // and compares the Lua-facing scalar the reference passes to `SetRotation`.
    if booth.turn.faced != Some(yaw) {
        if let Some(prev) = booth.turn.faced {
            booth.turn.spun = Some(booth::turn_shuffle(prev, yaw));
        }
        booth.turn.faced = Some(yaw);
    }
    if parts_changed || *last_pose != Some((yaw, model_scale)) {
        commands.entity(booth.root).insert(
            Transform::from_rotation(Quat::from_rotation_y(yaw))
                .with_scale(Vec3::splat(model_scale)),
        );
        *last_pose = Some((yaw, model_scale));
        booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
    }
}

/// What one frame owes a booth's pipeline settle ([`Booth::pipes_settling`]).
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
enum PipeSettle {
    /// Variants are still building, and dropping draws: keep the camera awake.
    Hold,
    /// Drained: spend the settle on one final rendered frame.
    Spend,
    /// Idle, but the settle window has not drained, so the reading predates the bake.
    Idle,
    /// [`PIPELINE_SETTLING_SECS`] ran out: spend it anyway.
    Expired,
}

/// A bake's variants are queued by the render world a frame after the spawn and counted a frame
/// later, so `Spend` waits for the [`BOOTH_SETTLE_FRAMES`] window to drain.
fn pipe_settle(compiling: bool, wake_drained: bool, held_for: f64) -> PipeSettle {
    if held_for > PIPELINE_SETTLING_SECS {
        PipeSettle::Expired
    } else if compiling {
        PipeSettle::Hold
    } else if wake_drained {
        PipeSettle::Spend
    } else {
        PipeSettle::Idle
    }
}

/// The demand-render gate: a booth camera renders only for [`Booth::wake`] frames after a content
/// edge, while a texture or pipeline is pending, or while its content is live (the glue scene, a
/// drawn body pane), and throughout the warm pass, which compiles the booths' pipelines.
fn gate_booth_cameras(
    mut commands: Commands,
    mut booths: ResMut<Booths>,
    preview: Res<GluePreview>,
    panes: Res<BoothPanes>,
    images: Res<Assets<Image>>,
    warm: Res<crate::pipe_warm::WarmPass>,
    cover: Res<crate::loading_screen::EntryCover>,
    pipes: Res<crate::pipe_warm::PipeWatch>,
    time: Res<Time<bevy::time::Real>>,
    mut cams: Query<(
        Entity,
        &BoothCam,
        &mut Camera,
        Has<benilla_world::particles::ViewThrottled>,
    )>,
    rate: Res<PaneRate>,
    frames: Res<bevy::diagnostic::FrameCount>,
    // `WOW_BOOTH_LOG` only: the marker's real state beside `booth.parked`.
    markers: Query<(), With<benilla_world::rig_anim::AnimParked>>,
    mut env_cache: Local<Option<bool>>,
) {
    let test = test_mode(&mut env_cache);
    // Only once the loading cover is on the glass: the flip frame owes it a loading screen,
    // not fifteen booth passes.
    let warming = !warm.satisfied() && cover.presented();
    for (cam_entity, BoothCam(token), mut cam, was_paced) in &mut cams {
        let Some(booth) = booths.0.get_mut(token.as_str()) else {
            continue;
        };
        let had_pending = !booth.pending.is_empty();
        booth.pending.retain(|h| !images.contains(h));
        if had_pending && booth.pending.is_empty() {
            booth.wake = booth.wake.max(1);
        }
        // Bound the hold: a texture that never lands must not keep the camera rendering.
        if booth.pending.is_empty() {
            booth.pending_since = None;
        } else {
            let now = time.elapsed_secs_f64();
            let since = *booth.pending_since.get_or_insert(now);
            if now - since > PENDING_LANDING_SECS {
                warn!(
                    "booth {}: {} texture(s) never landed after {PENDING_LANDING_SECS:.0}s — \
                     releasing the wake hold with the still as-is",
                    token.as_str(),
                    booth.pending.len(),
                );
                booth.pending.clear();
                booth.pending_since = None;
                booth.wake = booth.wake.max(1);
            }
        }
        // Judged only once the settle window drains, since the counters lag the bake by a frame.
        let settling = if booth.pipes_settling {
            let now = time.elapsed_secs_f64();
            let since = *booth.pipes_since.get_or_insert(now);
            match pipe_settle(pipes.compiling(), booth.wake == 0, now - since) {
                PipeSettle::Hold => true,
                PipeSettle::Idle => false,
                spent => {
                    if spent == PipeSettle::Expired {
                        warn!(
                            "booth {}: pipelines still compiling after \
                             {PIPELINE_SETTLING_SECS:.0}s — spending the settle with the still \
                             as-is",
                            token.as_str(),
                        );
                    }
                    booth.pipes_settling = false;
                    booth.wake = booth.wake.max(1);
                    false
                }
            }
        } else {
            booth.pipes_since = None;
            false
        };
        let live_scene = token.as_str() == GLUE_SLOT && preview.scene.is_some();
        // The glue screens publish no pane and stay on `live_scene`.
        let live_pane = booth.live && panes.0.contains_key(token.as_str());
        let active = test
            || warming
            || live_scene
            || live_pane
            || booth.wake > 0
            || !booth.pending.is_empty()
            || settling;
        // Half-rate (`PaneRate`): skip every other frame when only the live pane keeps this camera
        // up. `paced` marks the regime for the emitter lane (`ViewThrottled`), so a skipped render
        // is not read as a stopped scene.
        let paced = rate.half
            && live_pane
            && !(test || warming || live_scene)
            && booth.wake == 0
            && booth.pending.is_empty()
            && !settling;
        let throttled = paced && frames.0 % 2 == 1;
        let render = active && !throttled;
        if was_paced != paced {
            if paced {
                commands
                    .entity(cam_entity)
                    .insert(benilla_world::particles::ViewThrottled);
            } else {
                commands
                    .entity(cam_entity)
                    .remove::<benilla_world::particles::ViewThrottled>();
            }
        }
        // `WOW_BOOTH_LOG=1`: every activity flip and armed frame.
        static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *LOG.get_or_init(|| std::env::var_os("WOW_BOOTH_LOG").is_some())
            && (cam.is_active != render || active)
        {
            eprintln!(
                "[booth] t={:7.2} {} active={} render={} wake={} pending={} settling={} \
                 marker={}",
                time.elapsed_secs(),
                token.as_str(),
                active,
                render,
                booth.wake,
                booth.pending.len(),
                settling,
                markers.contains(booth.root),
            );
        }
        if cam.is_active != render {
            cam.is_active = render;
        }
        // Park the scene with its camera. This runs before the PostUpdate animation lane, so the
        // wake window's first render already animates.
        if booth.parked == active && booth.rigged {
            if active {
                commands
                    .entity(booth.root)
                    .remove::<benilla_world::rig_anim::AnimParked>();
            } else {
                commands
                    .entity(booth.root)
                    .insert(benilla_world::rig_anim::AnimParked);
            }
            booth.parked = !active;
        }
        if active {
            booth.wake = booth.wake.saturating_sub(1);
        }
    }
}

const TEMPORARY_PORTRAIT: &str = "Interface\\CharacterFrame\\TemporaryPortrait";

/// The 2D stand-in for a unit that is not yet renderable: the player file is `0x525ba0`'s.
fn temporary_portrait(net: Option<&NetEntity>, store: Option<&crate::net::ObjectStore>) -> String {
    use benilla_protocol::EntityKind;
    if net.map(|n| n.kind) == Some(EntityKind::Player) {
        return match store {
            Some(s) => player_temporary_portrait(s.0.unit_race(), s.0.unit_gender()),
            None => format!("{TEMPORARY_PORTRAIT}.blp"),
        };
    }
    creature_temporary_portrait()
}

/// The player stand-in from race and sex bytes, which an out-of-area party member gets from the
/// name cache. An unknown race gives the plain file, as the reference's `%s-%s` format does.
fn player_temporary_portrait(race: Option<u8>, sex: Option<u8>) -> String {
    let sex = match sex {
        Some(1) => "Female",
        _ => "Male",
    };
    let race = match race {
        Some(1) => "Human",
        Some(2) => "Orc",
        Some(3) => "Dwarf",
        Some(4) => "NightElf",
        Some(5) => "Scourge",
        Some(6) => "Tauren",
        Some(7) => "Gnome",
        Some(8) => "Troll",
        _ => return format!("{TEMPORARY_PORTRAIT}.blp"),
    };
    format!("{TEMPORARY_PORTRAIT}-{sex}-{race}.blp")
}

/// The creature stand-in, benilla's own file: the reference's only creature stand-in is `-Pet`,
/// for a roster pet with no object (`0x525cb0`), and a live creature not yet loaded shows blank.
fn creature_temporary_portrait() -> String {
    format!("{TEMPORARY_PORTRAIT}-Monster.blp")
}

/// Set the named slot's camera transform and projection to `rig`.
fn aim(
    cams: &mut Query<(&BoothCam, &mut Transform, &mut Projection)>,
    token: &str,
    rig: &(Transform, Projection),
) {
    for (cam, mut t, mut p) in cams.iter_mut() {
        if cam.0 == token {
            *t = rig.0;
            *p = rig.1.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::ItemModelKind;

    #[test]
    fn the_bake_cache_replaces_in_place_and_evicts_oldest_first() {
        let mut bakes = PortraitBakes::default();
        let mut images = Assets::<Image>::default();
        let face = |images: &mut Assets<Image>| images.add(new_target_image(1));

        let faces: Vec<Handle<Image>> =
            (0..PortraitBakes::CAP).map(|_| face(&mut images)).collect();
        for (i, f) in faces.iter().enumerate() {
            bakes.store(i as u64 + 1, f.clone());
        }
        // A replace does not move up the queue (the reference re-stores `+0x24`).
        let rebaked = face(&mut images);
        bakes.store(1, rebaked.clone());
        assert_eq!(bakes.get(1), Some(rebaked));
        assert_eq!(
            bakes.order.len(),
            PortraitBakes::CAP,
            "a replace is not an insert"
        );

        let newcomer = face(&mut images);
        bakes.store(99, newcomer.clone());
        assert_eq!(bakes.get(1), None, "the oldest face went");
        assert_eq!(bakes.get(2), Some(faces[1].clone()), "and only the oldest");
        assert_eq!(bakes.get(99), Some(newcomer));
        assert_eq!(bakes.faces.len(), PortraitBakes::CAP);
    }

    fn body_part() -> PortraitPart {
        PortraitPart {
            static_mesh: Handle::default(),
            skinned_mesh: None,
            material: Handle::default(),
        }
    }

    fn card(seat: PortraitSeat) -> PortraitBillboard {
        card_at(seat, Some(SHOULDER))
    }

    fn card_at(seat: PortraitSeat, attach: Option<u16>) -> PortraitBillboard {
        PortraitBillboard {
            mesh: Handle::default(),
            material: Handle::default(),
            bone: 4,
            seat,
            kind: benilla_formats::BillboardKind::Spherical,
            attach,
        }
    }

    /// A worn attach id the reset keeps (the right pauldron).
    const SHOULDER: u16 = 5;

    fn rider() -> PortraitRider {
        rider_at(SHOULDER)
    }

    fn rider_at(attach: u16) -> PortraitRider {
        PortraitRider {
            static_mesh: Handle::default(),
            material: Handle::default(),
            bone: 4,
            offset: Vec3::new(0.21, 1.42, 0.06),
            attach: Some(attach),
        }
    }

    fn effects(bone: u16, offset: Vec3, count: usize) -> PortraitEffects {
        effects_at(bone, offset, count, SHOULDER)
    }

    fn effects_at(bone: u16, offset: Vec3, count: usize, attach: u16) -> PortraitEffects {
        PortraitEffects {
            bone,
            offset,
            attach: Some(attach),
            emitters: (0..count)
                .map(|_| benilla_assets::ModelEmitter {
                    def: benilla_world::testing::plain_particle_def(),
                    texture: None,
                    bone_pivot: [0.0; 3],
                    billboard: None,
                    recursion: None,
                    geometry: None,
                    owner_reach: 0.0,
                    water_bound: (Vec3::ZERO, 0.0),
                    idle_seq: 0,
                })
                .collect(),
        }
    }

    #[derive(Resource, Default)]
    struct Collected {
        parts: usize,
        riders: usize,
        billboards: Vec<PortraitSeat>,
        effects: Vec<(u16, usize)>,
        grip: [bool; 2],
    }

    fn walk(app: &mut App, unit: Entity) -> Collected {
        app.init_resource::<Collected>();
        app.insert_resource(Unit(unit));
        app.add_systems(
            Update,
            |unit: Res<Unit>, look: DressedLook, mut out: ResMut<Collected>| {
                let (parts, riders, billboards, effects) = look.collect(unit.0);
                *out = Collected {
                    parts: parts.len(),
                    riders: riders.len(),
                    billboards: billboards.iter().map(|b| b.seat).collect(),
                    effects: effects.iter().map(|e| (e.bone, e.emitters.len())).collect(),
                    grip: hand_grip(&riders, &billboards, &effects),
                };
            },
        );
        app.update();
        std::mem::take(app.world_mut().resource_mut::<Collected>().as_mut())
    }

    #[derive(Resource)]
    struct Unit(Entity);

    #[test]
    fn a_mounts_own_glow_card_prunes_but_the_riders_gear_does_not() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let unit = app.world_mut().spawn(body_part()).id();
        app.world_mut()
            .spawn((card(PortraitSeat::Body), ChildOf(unit)));
        let mount = app
            .world_mut()
            .spawn((
                body_part(),
                crate::entities::mount::MountBody { host: unit },
                ChildOf(unit),
            ))
            .id();
        app.world_mut()
            .spawn((card(PortraitSeat::Body), ChildOf(mount)));
        // The seated rider's gear, re-rooted under the mount.
        let seat = app.world_mut().spawn(ChildOf(mount)).id();
        app.world_mut().spawn((rider(), ChildOf(seat)));
        app.world_mut().spawn((
            card(PortraitSeat::Rider(Vec3::new(0.15, 1.58, 0.05))),
            ChildOf(seat),
        ));
        app.world_mut()
            .spawn((effects(4, Vec3::new(0.21, 1.42, 0.06), 2), ChildOf(seat)));

        let got = walk(&mut app, unit);
        assert_eq!(got.parts, 1, "the character's body, never the mount's");
        assert_eq!(
            got.riders, 1,
            "the rider's shoulder survives the mount subtree"
        );
        let (body, rider): (Vec<PortraitSeat>, Vec<PortraitSeat>) = got
            .billboards
            .iter()
            .partition(|s| **s == PortraitSeat::Body);
        assert_eq!(
            body.len(),
            1,
            "the character's eye-glow, not the mount's lantern"
        );
        assert_eq!(
            rider,
            vec![PortraitSeat::Rider(Vec3::new(0.15, 1.58, 0.05))],
            "the item's card survives the mount subtree, like the rider it belongs to",
        );
        assert_eq!(
            got.effects,
            vec![(4, 2)],
            "an item's emitters are never pruned"
        );
    }

    fn mesh(n: u64) -> Handle<Mesh> {
        Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(0xb324_0000 | u128::from(n)),
            std::marker::PhantomData,
        )
    }
    fn mat(n: u64) -> Handle<WowModelMaterial> {
        Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(0xb324_1000 | u128::from(n)),
            std::marker::PhantomData,
        )
    }

    fn geom_rider(attach: u16, n: u64) -> PortraitRider {
        PortraitRider {
            static_mesh: mesh(n),
            material: mat(n),
            bone: 4,
            offset: Vec3::ZERO,
            attach: Some(attach),
        }
    }

    fn dress_of(held: [Option<(u32, ItemModelKind, i32)>; 3]) -> crate::entities::DressKey {
        crate::entities::DressKey {
            display_id: Some(57),
            held,
            held_ready: true,
        }
    }

    fn hunter() -> crate::entities::DressKey {
        dress_of([None, None, Some((3026, ItemModelKind::Weapon, 0))])
    }

    fn key(
        riders: &[PortraitRider],
        dress: crate::entities::DressKey,
        rev: u32,
        show: u32,
    ) -> SnapKey {
        let body = [body_part()];
        let body: Vec<&PortraitPart> = body.iter().collect();
        let riders: Vec<&PortraitRider> = riders.iter().collect();
        SnapKey::build(
            Entity::PLACEHOLDER,
            &body,
            &riders,
            &[],
            &[],
            Some(dress),
            rev,
            show,
            1.0,
        )
    }

    /// The combat auto-draw is a snap (`bInstant != 0`), reaching `UNIT_MODEL_CHANGED` only
    /// through the enchant-gated `0x5eed50`.
    #[test]
    fn drawing_a_bow_in_combat_does_not_re_snapshot_the_pane() {
        use crate::entities::attach_id::HAND_LEFT;
        let bow = [geom_rider(HAND_LEFT, 1)];
        let stowed = key(&[], hunter(), 0, 1);
        let drawn = key(&bow, hunter(), 0, 1);
        assert!(
            stowed == drawn,
            "the world drew the bow; the widget's duplicate must not notice"
        );

        // The control: the mirrored-geometry key does move on the draw.
        let body = [body_part()];
        let body: Vec<&PortraitPart> = body.iter().collect();
        let bow: Vec<&PortraitRider> = bow.iter().collect();
        assert!(
            LookKey::build(&body, &[], &[], &[]) != LookKey::build(&body, &bow, &[], &[]),
            "the pre-1616 key moved on the draw — which is the bug"
        );
    }

    #[test]
    fn stowing_a_sword_does_not_re_snapshot_the_pane() {
        use crate::entities::attach_id::{HAND_RIGHT, HIP_MAIN};
        let sword = dress_of([Some((1234, ItemModelKind::Weapon, 0)), None, None]);
        let drawn = key(&[geom_rider(HAND_RIGHT, 7)], sword, 0, 1);
        let hipped = key(&[geom_rider(HIP_MAIN, 7)], sword, 0, 1);
        assert!(drawn == hipped);
    }

    /// `ToggleSheath` passes `bInstant = 0`, and the ceremony's keyframe marks the unit
    /// (`0x5ffb10`).
    #[test]
    fn the_manual_sheath_ceremony_re_snapshots_the_pane() {
        use crate::entities::attach_id::HAND_LEFT;
        let before = key(&[], hunter(), 0, 1);
        let after = key(&[geom_rider(HAND_LEFT, 1)], hunter(), 1, 1);
        assert!(
            before != after,
            "the ceremony's own keyframe is a model event"
        );
    }

    /// The show override `0x505d00`, not a Lua event.
    #[test]
    fn showing_the_pane_re_snapshots_it() {
        use crate::entities::attach_id::HAND_LEFT;
        let open_stowed = key(&[], hunter(), 0, 1);
        let reopened_drawn = key(&[geom_rider(HAND_LEFT, 1)], hunter(), 0, 2);
        assert!(open_stowed != reopened_drawn);
    }

    /// Equipping marks the unit (`0x5e2810` → `0x5dee30` → `0x5df119`).
    #[test]
    fn equipping_a_stowed_weapon_still_re_snapshots_the_pane() {
        let empty = key(&[], dress_of([None, None, None]), 0, 1);
        let armed = key(&[], hunter(), 0, 1);
        assert!(
            empty != armed,
            "the dress moved even though nothing is drawn"
        );
    }

    #[test]
    fn a_helm_arriving_re_snapshots_the_pane() {
        use crate::entities::attach_id::HELM;
        let bare = key(&[], hunter(), 0, 1);
        let helmed = key(&[geom_rider(HELM, 9)], hunter(), 0, 1);
        assert!(bare != helmed);
    }

    /// `0x47a230` detaches the nocked arrow (`0x23`) and keeps the hand points.
    #[test]
    fn the_nocked_arrow_never_reaches_a_booth_but_the_bow_in_hand_does() {
        use crate::entities::attach_id::{HAND_ARROW, HAND_LEFT};
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let unit = app.world_mut().spawn(body_part()).id();
        app.world_mut().spawn((rider_at(HAND_LEFT), ChildOf(unit)));
        app.world_mut().spawn((rider_at(HAND_ARROW), ChildOf(unit)));

        let got = walk(&mut app, unit);
        assert_eq!(
            got.riders, 1,
            "the arrow is detached from the duplicate; the bow is not"
        );
        assert_eq!(
            got.grip,
            [false, true],
            "a bow at HandLeft closes the LEFT hand (0x5059a0's occupancy probe), \
             and nothing closes the right"
        );
    }

    #[test]
    fn the_attach_reset_cuts_the_transient_family_and_keeps_equipment() {
        for kept in [
            0, 1, 2, // the forearm and the two hands
            3, 4, 5, 6, 7, 8, 9, 10, // elbows, shoulders, knees, hips
            0xb, 0xc, 0xd, 0xe, // helm, cape, the two shoulder flaps
            0x1a, 0x1b, 0x1c, // sheath main/off, sheath shield (also the worn quiver, 0x1a)
            0x1e, 0x1f, 0x20, 0x21, // the large-weapon and hip-weapon sheath pairs
        ] {
            assert!(!attach_reset(Some(kept)), "0x47a230 keeps attach {kept:#x}");
        }
        for cut in [
            0xf, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1d, 0x22, 0x23,
        ] {
            assert!(attach_reset(Some(cut)), "0x47a230 detaches attach {cut:#x}");
        }
        assert!(
            !attach_reset(None),
            "the host model's OWN batches sit in no attachment node — the reset walks the \
             attachment list and cannot reach them"
        );
    }

    /// `0x479700`/`0x5059a0` close hands on ids `1`/`2` alone (`0x60b678`).
    #[test]
    fn a_shield_on_the_forearm_closes_no_hand() {
        use crate::entities::attach_id::{HAND_RIGHT, SHIELD};
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let unit = app.world_mut().spawn(body_part()).id();
        app.world_mut().spawn((rider_at(SHIELD), ChildOf(unit)));
        app.world_mut().spawn((rider_at(HAND_RIGHT), ChildOf(unit)));

        assert_eq!(
            walk(&mut app, unit).grip,
            [true, false],
            "the sword's hand closes, the shield's does not"
        );
    }

    /// The reference probes the attachment node, which every lane hangs from.
    #[test]
    fn a_meshless_held_item_still_closes_its_hand() {
        use crate::entities::attach_id::{HAND_LEFT, HAND_RIGHT};
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let unit = app.world_mut().spawn(body_part()).id();
        app.world_mut().spawn((
            card_at(PortraitSeat::Rider(Vec3::ZERO), Some(HAND_RIGHT)),
            ChildOf(unit),
        ));
        app.world_mut()
            .spawn((effects_at(4, Vec3::ZERO, 1, HAND_LEFT), ChildOf(unit)));

        assert_eq!(walk(&mut app, unit).grip, [true, true]);
    }

    #[test]
    fn a_glow_arriving_after_its_item_still_changes_the_bake_key() {
        let parts = [body_part()];
        let riders = [rider()];
        let parts: Vec<&PortraitPart> = parts.iter().collect();
        let riders: Vec<&PortraitRider> = riders.iter().collect();

        let before = LookKey::build(&parts, &riders, &[], &[]);
        let glow = effects(4, Vec3::new(0.21, 1.42, 0.06), 1);
        let after = LookKey::build(&parts, &riders, &[], &[&glow]);
        assert!(before != after, "the glow's arrival is a content edge");

        // Two glow slots on one weapon differ by the seat alone.
        let moved = effects(4, Vec3::new(0.21, 1.55, 0.06), 1);
        assert!(
            LookKey::build(&parts, &riders, &[], &[&moved]) != after,
            "the seat is part of the key",
        );
        assert!(LookKey::build(&parts, &riders, &[], &[&glow]) == after);
    }

    /// The stand-in's player arm (`0x525ba0`).
    #[test]
    fn the_stand_in_names_the_sex_and_race_and_falls_back_without_them() {
        assert_eq!(
            player_temporary_portrait(Some(4), Some(1)),
            "Interface\\CharacterFrame\\TemporaryPortrait-Female-NightElf.blp",
        );
        assert_eq!(
            player_temporary_portrait(Some(6), Some(0)),
            "Interface\\CharacterFrame\\TemporaryPortrait-Male-Tauren.blp",
        );
        // The plain file, as the reference's `%s-%s` format degenerates.
        assert_eq!(
            player_temporary_portrait(None, None),
            "Interface\\CharacterFrame\\TemporaryPortrait.blp",
        );
        assert_eq!(
            player_temporary_portrait(Some(9), Some(1)),
            "Interface\\CharacterFrame\\TemporaryPortrait.blp",
        );
    }

    /// Off macOS a not-yet-built variant draws nothing, so a still committed then keeps the hole.
    #[test]
    fn a_bake_holds_its_camera_awake_while_pipelines_are_still_building() {
        assert_eq!(pipe_settle(true, true, 0.0), PipeSettle::Hold);
        assert_eq!(pipe_settle(true, false, 0.0), PipeSettle::Hold);
    }

    #[test]
    fn an_idle_reading_before_the_settle_window_drains_decides_nothing() {
        assert_eq!(pipe_settle(false, false, 0.0), PipeSettle::Idle);
        assert_eq!(pipe_settle(false, true, 0.0), PipeSettle::Spend);
    }

    #[test]
    fn the_settle_is_bounded_even_while_the_cache_keeps_filling() {
        assert_eq!(
            pipe_settle(true, false, PIPELINE_SETTLING_SECS + 0.1),
            PipeSettle::Expired,
        );
        assert_eq!(
            pipe_settle(true, true, PIPELINE_SETTLING_SECS + 0.1),
            PipeSettle::Expired,
        );
    }
}
