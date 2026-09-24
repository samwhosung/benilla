//! Unit-frame **portraits** — the modern 2D take on the player/target face windows.
//!
//! The real 1.12 client renders a unit's model **once** into a tiny (64²) off-screen texture and
//! freezes it (re-baked only on model change), then stamps a round alpha stencil into it; the low
//! resolution is a 2004 shortcut, not a look (`0x524f60`).
//! benilla keeps the *idea* — a flat 2D face in the ring — but bakes it properly: a **high-resolution**
//! off-screen render of the unit's real model, so the still is crisp. No live 3D widget sits in the UI;
//! what lands in the circle is a plain rendered image (director's call).
//!
//! ## The "photo booth"
//!
//! Each portrait slot (`"player"`/`"target"`) gets its own render layer + camera rendering into a
//! [`PortraitImages`] entry. A third slot — `"paperdoll"` (decision 0208 §5) — reuses the exact same
//! bake pipeline for the character window's **full-body** model pane: it mirrors the *player's*
//! dressed look like the `"player"` slot, but frames it through the model's own `<PlayerModel>`
//! camera ([`framing::body_frame`] — raw `cameras[1]`, not the authored bust camera the round
//! portraits use; decision 1089), bakes at 512², and spins the
//! model to a live yaw ([`PaperDollBooth`], the ref's `Model:SetRotation`). The UI samples it
//! *square*, not through the circular mask.
//!
//! The parts baked are the unit's **live dressed look** — the attach path's
//! spawned children (geosets already appearance-filtered; materials already carrying the composited
//! body skin / hair / NPC atlas), each stamped with a [`PortraitPart`] naming its static bind-pose
//! mesh twin + steady exterior material. Mirroring the children (not the shared display cache) means
//! the portrait can never drift from what's standing in the world, and gear/appearance rebuilds
//! re-bake automatically (the parts key changes). While a live unit's model is still loading, the
//! slot shows the ref's own 2D stand-in (`TemporaryPortrait-{Sex}-{Race}` / `-Monster`,
//! `0x525ba0`) via [`PortraitSource::File`].
//!
//! ## Framing: the model's own authored camera
//!
//! The framing is the model's **authored portrait camera** — the MD20 camera `cameraLookup[0]`
//! selects (`0x713540`): the real bake builds `lookAt(eye, target, up-from-roll)` + the gxumath
//! *diagonal-FOV* perspective at the portrait path's aspect, which is **1.0 on every screen** —
//! net vertical half-angle `fov/(2√2)`, isotropic (`framing::WowPortraitProjection`, decision
//! 1543) — and **no** engine-side yaw or normalization on top. Every artist calibrated camera 0 to their
//! own model — that is the whole mechanism behind the ref's uniformly tight, consistently-angled
//! face crops across humans, wolves, and rabbits. A camera-less model (a few creatures,
//! props) falls back to [`frame`]'s heuristic head-anchor framing.
//!
//! ## Pose: a fresh instance at Stand
//!
//! The bake is **posed like the ref's** (instance `0x707400`, pose `0x7121a0`): a fresh throwaway
//! instance — the booth's own joint hierarchy + the parts' skinned twins — armed to the model's
//! Stand (anim id 0 through its own baked resolution, the ref's loader-idle seed) and frozen, never
//! the unit's live world pose. Bone riders (helm/shoulder armor, held items) ride their bone's
//! joint, so they sit in the Stand pose exactly like the world instance ([`PortraitRider`]; the ref
//! resets the attach *sockets* the same way, `0x47a230`). See [`spawn_booth_model`].
//!
//! ## Deviations from the ref (deliberate) and what's still coarse
//!
//! High-res (256² vs 64²), a fixed neutral **studio light** on the round portraits (vs the ref's
//! ambient state — the *body* panes instead carry the reference's own `<PlayerModel>` light, see
//! [`model_pane_light`]), continuous
//! booth render vs dirty-byte bake, and the frozen Stand *phase* is t=0 (the ref's sampling clock
//! is the one unsettled, inferred point — t≈0 vs live phase; both are Stand, and t=0
//! reproduces the ref wolf's open mouth). The creature *loading* stand-in is `-Monster` (our
//! pick — the ref's `-Pet` belongs to its pet-frame delegate).
//! `WOW_PORTRAIT_TEST=<Model\Path.mdx>` (+ `WOW_PORTRAIT_TEST_SKIN=<blp>`) bakes that model into
//! every slot (both portraits + the paper doll's body framing) to eyeball the pipeline without a server.

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
/// The translucency twins ride the *preview* shapes as well as the booth ones — one type for both
/// halves of the same batch, so the entities-side assembly and the bake cannot disagree about what
/// a batch can feather through.
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

/// The portrait slots we bake, each with its own render layer/camera: the player + target unit
/// frames, `"pet"` (decision 0990's frame), and `"npc"` — the NPC an interaction window (gossip /
/// quest / merchant / trainer / taxi) is bound to ([`crate::ui_session::InteractNpc`]), so those
/// windows show the creature's face instead of the `?` placeholder.
/// `"targettarget"` (decision 1576) is the ninth and the odd one: it is the only slot whose unit
/// resolution is gated on the UI actually drawing it — see [`sync_portraits`]'s arm for why.
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
/// The character-window **paper-doll** slot (decision 0208 §5): a full-body bake of the dressed
/// player, sampled *square* (not circular) by the character frame's model pane. Its own booth —
/// separate resolution ([`PAPERDOLL_SIZE`]), body framing ([`framing::body_frame`]), and a live yaw
/// ([`PaperDollBooth`]) — so the two portrait slots stay pixel-identical.
const PAPERDOLL_SLOT: &str = "paperdoll";
/// The **inspect** window's model pane (decision 0631 §4) — the same full-body bake as
/// [`PAPERDOLL_SLOT`], pointed at *another* player. It is the composition of what the existing
/// slots already do separately: body framing like the paper doll, an arbitrary unit like
/// `"target"`. Both run through [`sync_body_booth`]; only the unit and the yaw differ.
const INSPECT_SLOT: &str = "inspect";
/// The **pet paper doll**'s model pane (decision 1057) — the character window's tab 2. Composed
/// exactly like [`INSPECT_SLOT`] (body framing + an arbitrary unit + a live yaw), pointed at the
/// pet instead of at another player; it too runs through [`sync_body_booth`]. It gets its own booth
/// rather than sharing the `"pet"` portrait slot because that one is a 256² *bust* for the pet unit
/// frame — different subject framing, different resolution, and a yaw the portrait must never have.
const PETDOLL_SLOT: &str = "petdoll";
/// The **stable window**'s model pane (`0x4cb870`) — the
/// fourth body pane, and the only one whose subject may have no world object at all.
/// `SetPetStablePaperdoll` forks on the selection: `-1` (the summoned pet) resolves the live pet by
/// GUID and takes its model, anything else maps the petNumber back to a slot and goes through the
/// **creature cache**, i.e. a bare `CreatureDisplayInfo` id for a pet that is asleep in a stable and
/// has no unit anywhere. Both forks land in the same booth here: the live one points
/// [`StableBooth::unit`] at the pet entity ([`PETDOLL_SLOT`]'s case exactly), the cached one points
/// [`StableBooth::display_id`] at the display and the mirror comes off a [`PortraitStandIn`].
const STABLE_SLOT: &str = "stable";

/// **Which booth a `<Model>`/`<PlayerModel>` pane samples** — the one join between the reference's
/// own frame names and this module's body-bake slots.
///
/// The reference draws a body pane by rendering the unit into the widget live; benilla renders it
/// into an off-screen bake this module keeps *per window* (the doc above: framing, resolution and
/// yaw differ per pane, so they are separate booths). Which bake a pane samples is therefore a
/// fact about the window, not about the widget — and the pane's global name is what identifies the
/// window, because since decision 1751 that name is the reference's own.
///
/// This table replaces `BenillaSetBoothTexture(region, token)`, the Lua binding that used to carry
/// the same join: a file of ours put a Texture inside the pane and named the slot. A migrated
/// window's file is the reference's, so the join cannot live in Lua any more — and it should not,
/// because a stock addon creating a `<PlayerModel>` never called it either. A pane that is not in
/// this table draws nothing, exactly as it does today.
///
/// The names, and where each is declared: `CharacterModelFrame` (stock `PaperDollFrame.xml`),
/// `PetModelFrame` (stock `PetPaperDollFrame.xml`), `PetStableModel` (stock `PetStable.xml`, the
/// reference's own file since 1751), `DressUpModel` (stock `DressUpFrame.xml`, 1969),
/// `AuctionDressUpModel` (the auction addon's `Blizzard_AuctionDressUp.xml`, 1971), and
/// `InspectModelFrame` (stock `InspectPaperDollFrame.xml`, out of the LoadOnDemand
/// `Blizzard_InspectUI` — the reference's own since 1832).
///
/// **Every name here is now the reference's.** This note used to carry a `BenillaInspectModelFrame`
/// and an explanation: the install was thought to ship that addon packed, with no stock file to
/// take the name from. It is not packed — the `.pub` is only what the loose
/// `Interface\AddOns\Blizzard_InspectUI\` folder holds, while the real `.xml` and `.lua` sit
/// inside `patch.MPQ`, which the chain mounts. 1832 migrated the window and the prefix went with it.
const MODEL_PANE_BOOTHS: [(&str, &str); 7] = [
    ("CharacterModelFrame", PAPERDOLL_SLOT),
    // The tabard designer's pane (1977): a PlayerModel of the player — the paper doll's own
    // bake, whose body wears the design under preview while the window is up.
    ("TabardModel", PAPERDOLL_SLOT),
    ("PetModelFrame", PETDOLL_SLOT),
    ("InspectModelFrame", INSPECT_SLOT),
    ("PetStableModel", STABLE_SLOT),
    ("DressUpModel", dressup::DRESSUP_SLOT),
    // The auction house's embedded dressing room (`Blizzard_AuctionDressUp.xml`, 1971) — a second
    // `<DressUpModel>` sharing the one booth: the app keeps ONE dressing-room look (its
    // `TryOn`/`Dress` intents are not per-widget), so the two panes show the same substitutions.
    // A deliberate approximation, named: the reference's two widgets each clone their own model.
    ("AuctionDressUpModel", dressup::DRESSUP_SLOT),
];

/// The booth a named model pane samples, or `None` for a pane no window has claimed.
pub(crate) fn model_pane_booth(name: &str) -> Option<&'static str> {
    MODEL_PANE_BOOTHS
        .iter()
        .find(|(pane, _)| *pane == name)
        .map(|(_, slot)| *slot)
}

/// World is layer 0, the UI quad pass layer 1; portraits sit on their own high layers so nothing in the
/// world leaks into a booth and vice-versa (one layer per slot: base, base+1, …).
const PORTRAIT_LAYER_BASE: usize = 2;
/// The paper-doll booth's render layer — the next layer past the portrait slots.
const PAPERDOLL_LAYER: usize = PORTRAIT_LAYER_BASE + SLOTS.len();
/// The inspect booth's render layer — the next one past the paper doll's.
const INSPECT_LAYER: usize = PAPERDOLL_LAYER + 1;
/// The pet-doll booth's render layer — the next one past inspect's (the one ladder below).
const PETDOLL_LAYER: usize = INSPECT_LAYER + 1;
/// The stable pane's render layer — the next one past the pet doll's, by the ladder rule below.
const STABLE_LAYER: usize = PETDOLL_LAYER + 1;
/// The glue booth's render layer — the next one past inspect's. **Every booth layer is computed
/// HERE**, in one ladder, because they were not: the glue booth (`930b327c`) and the inspect booth
/// (`ead3b0c9`) each defined "the next layer past the paper doll's" in a different file and landed
/// on the same number. Two booths sharing a layer is not a cosmetic clash — `particles::sim`
/// resolves an emitter's booth camera by *finding the first camera whose layers intersect*, so the
/// glue scene's 28 emitters addressed the INSPECT camera, which is off at the glue screens, and the
/// login screen's braziers simulated forever without ever being drawn (decision 0775).
pub(super) const GLUE_LAYER: usize = STABLE_LAYER + 1;
/// The **dressing room**'s render layer (decision 1060) — the next one past the glue booth's, by
/// the same ladder rule.
pub(super) const DRESSUP_LAYER: usize = GLUE_LAYER + 1;
/// The pipe_warm **twin booth**'s render layer — the next one past the dressing room's (same ladder
/// rule as [`GLUE_LAYER`]). This camera exists only while the warm pass runs
/// ([`spawn_warm_booth`]); nothing but menagerie rigs ever rides its layer.
pub(crate) const WARM_BOOTH_LAYER: usize = DRESSUP_LAYER + 1;
/// pipe_warm's **orthographic twin camera**'s layer ([`crate::ui_models::spawn_warm_tile_cam`],
/// decision 2262) — the next one past the twin booth's, same ladder rule. bevy_pbr keys a mesh
/// pipeline on the view's projection CLASS, and the tile atlas's one camera is
/// `Projection::Orthographic`: a THIRD class beside the world camera's Perspective and the twin
/// booth's custom `WowPortraitProjection`. Like [`WARM_BOOTH_LAYER`] this camera exists only while
/// the warm pass runs, and nothing but menagerie rigs ever rides its layer.
pub(crate) const WARM_ORTHO_LAYER: usize = WARM_BOOTH_LAYER + 1;
/// The **minimap interior composite**'s render layer (decision 1466) — the next one past the warm
/// booth's. Not a portrait booth, but it is an offscreen camera with its own layer, and 0775's rule
/// is that EVERY such layer is computed in this one ladder: the two booths that each worked out
/// "the next layer past the paper doll's" in their own file landed on the same number, and the
/// clash was silent in both rendering and the emitter→camera match.
pub(crate) const MINIMAP_COMPOSITE_LAYER: usize = WARM_ORTHO_LAYER + 1;
/// The UI model tiles' layer (`crate::ui_models`, decision 2008): every `<Model>` widget's M2
/// renders into one atlas through one camera on this layer.
pub(crate) const UI_MODELS_LAYER: usize = MINIMAP_COMPOSITE_LAYER + 1;
/// The base of the **perspective model panes'** layer block (decision 2027). A `<Model>` framed
/// by its file's own camera cannot share the tile atlas's one orthographic camera — it needs a
/// camera of its own, rendering into its own cell of the same atlas through a viewport — and one
/// camera per pane means one layer per pane, or every perspective camera would draw every other
/// pane's model over its cell. The block runs `BASE + i` for `i < UI_MODEL_CAM_LAYERS` and sits at
/// the TOP of the ladder, so it can be widened without colliding with anything above it.
pub(crate) const UI_MODEL_CAM_LAYER_BASE: usize = UI_MODELS_LAYER + 1;
/// How many perspective model panes can draw at once — the size of the layer block above and of
/// the camera pool beside it. Each is a full render pass into the atlas, and a UI showing eight
/// authored-camera 3-D scenes at once is already far past anything the reference's interface or
/// the addon corpus does; a ninth pane holds its cell and draws nothing, the same degrade the
/// atlas already makes for a tile that does not fit.
pub(crate) const UI_MODEL_CAM_LAYERS: usize = 8;

// The ladder must stay collision-free: a booth camera's layer is its identity for both rendering
// and the emitter→camera match, and the failure above was silent in both.
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
/// The baked image is high-res (vs the ref's 64²) — the crisp modern look. Square; the UI quad
/// shader cuts the inscribed circle at draw time (`ui_quad.wgsl`'s `circular`, the ref's stencil).
const PORTRAIT_SIZE: u32 = 256;
/// The paper-doll bake resolution. The pane draws ~233×224 points; at 2× hidpi that is ≈466×448, so
/// 512² covers it crisply with a little to spare (and, being sampled square, wants more than the
/// portraits' 256² for the taller full-body subject).
const PAPERDOLL_SIZE: u32 = 512;
/// Stamped on every spawned unit-model part child by the attach path ([`crate::entities`]): the
/// part's two mesh twins — the booth poses the **skinned** twin at Stand on its own throwaway
/// skeleton (the ref bake: instance `0x707400`, pose `0x7121a0`), the **static** bind-pose twin
/// serving the boneless fallback — and its **steady exterior material** (the child may currently
/// wear the appear-fade blend or interior variant; a portrait always wants the steady look). The
/// booth mirrors a unit's `PortraitPart` children — the exact dressed look standing in the world.
#[derive(Component)]
pub(crate) struct PortraitPart {
    pub(crate) static_mesh: Handle<Mesh>,
    /// `None` for a WMO-display part (never skins) — the booth then draws the static twin.
    pub(crate) skinned_mesh: Option<Handle<Mesh>>,
    pub(crate) material: Handle<WowModelMaterial>,
}

/// Stamped on every spawned **bone-rider** mesh child (helm / shoulder / held item —
/// [`crate::entities::equipment`]): the mesh + steady material like [`PortraitPart`], plus where it
/// sits — the body bone it rides and the attach-point offset under that bone. The posed booth
/// seats the rider under its throwaway skeleton's joint (so it rides the Stand pose exactly like
/// the world instance rides its gait); the ref resets the attach *sockets* the same way
/// (`0x47a230`).
///
/// It also rides a **mirror-only carrier** — a marker child that draws nothing — where the world
/// geometry it names is not a unit descendant we can stamp: an item glow's own render batches, which
/// the shared effect lane spawns under its own rig ([`crate::entities::item_glow`], decision 0822).
/// A booth bakes those at the bind pose, like any other rider.
#[derive(Component)]
pub(crate) struct PortraitRider {
    pub(crate) static_mesh: Handle<Mesh>,
    pub(crate) material: Handle<WowModelMaterial>,
    /// The body-skeleton bone the rider's joint entity belongs to.
    pub(crate) bone: u16,
    /// The attach point's Bevy-space offset under that bone ([`crate::entities::BoneAttach`]).
    pub(crate) offset: Vec3,
    /// The **M2 attachment id** this rider's model hangs from on the body — what
    /// [`attach_reset`] filters on. `None` where the geometry is the host model's own and sits
    /// in no attachment node at all (see [`PortraitBillboard::attach`]).
    pub(crate) attach: Option<u16>,
}

/// Stamped on a lightweight **anchor child** the attach path plants under a unit for each
/// camera-facing batch it dresses in. The visible world card is a *root-spawned* entity
/// ([`benilla_world::billboard::BillboardCard`]), never a unit descendant, so it can't be mirrored; this
/// marker rides the unit's tree purely so the portrait / paper-doll booths (which mirror the unit's
/// dressed descendants) can rebuild the batch as a booth card ([`BoothBillboardSpec`] +
/// [`booth::face_booth_billboards`]) — the same reconstruction the char-create glue path does from
/// its own parts ([`PreviewBillboard`]). Carries the centred quad, its material, where it sits, and
/// the billboard flag.
///
/// Three sources plant one, exactly as on the glue path (decision 0822):
///
/// - The character's own **eye-glow** (undead/night-elf, geoset 302 / geoset 0 — a fullbright quad
///   on the eye bone), planted by [`crate::entities::attach`].
/// - An **equipped item's** camera-facing batch — a wand's gem, the held torch's `GLOWWHITE32` halo
///   (270 of the 2681 `Item\` models author one), planted by
///   [`crate::entities::equipment`]'s attach.
/// - An **item glow's** batch (decision 0805) — `Spells\Enchantments\Sparkle_A.m2` is one additive
///   quad and nothing else — planted by [`crate::entities::item_glow`].
#[derive(Component)]
pub(crate) struct PortraitBillboard {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) material: Handle<WowModelMaterial>,
    /// The BODY bone whose booth joint the card seats on — the eye bone for the character's own
    /// glow, the item's attach-point bone for anything an item wears.
    pub(crate) bone: u16,
    pub(crate) seat: PortraitSeat,
    pub(crate) kind: benilla_formats::BillboardKind,
    /// The **M2 attachment id** the batch's model hangs from on the body, for [`attach_reset`].
    /// `None` for a batch of the host model ITSELF (the character's own eye-glow): the reference's
    /// reset walks the duplicate's *attachment list*, and a geoset batch of the body is not in it.
    pub(crate) attach: Option<u16>,
}

/// Whose model a mirrored camera-facing batch belongs to — which decides both where its pivot comes
/// from and whether a *mounted* unit's booth keeps it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum PortraitSeat {
    /// A batch of the **rigged host model** itself (the eye-glow). Its bone's joint frame already
    /// bakes the bone pivot (the 0130 rig identity), so it needs no offset — and it belongs to that
    /// model's body, so a mount's own glow card prunes with the mount's meshes (a portrait shows the
    /// rider alone, never the horse — decision 0441).
    Body,
    /// A batch of a **rig-less rider** — an equipped item's, an item glow's — at its seat in the
    /// bone's joint frame (Bevy axes): the attach point **plus the batch's own model-local pivot**,
    /// because with no rig nothing else will bake that pivot. Collected like a [`PortraitRider`]
    /// whatever the mount state: while mounted, the character's own joints re-root INSIDE the mount
    /// subtree, and the rider's gear must survive that.
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

/// Stamped on an **effect-bearing model** riding a unit — the equipped item whose own emitters are
/// its whole look (the R14 PVP pauldron's `SPARKLE` twinkle, the held torch's flame — decision
/// 0813) and the `ItemVisuals` glow a held weapon hangs on its own attachment points
/// (decision 0805). The world emitters are *free* entities the owner contract walks
/// ([`benilla_world::particles::spawn_emitter`]), never unit descendants, so — like [`PortraitBillboard`] —
/// this marker is how a booth learns they exist at all: the mirror carries the emitter records plus
/// the composed seat, and the booth spawns its own copies against its own camera
/// ([`booth::spawn_booth_effects`]). The glue path's [`PreviewEffects`] is the same carry assembled
/// from data instead of mirrored from entities.
///
/// Which booths act on it is a *fidelity* split, not a cost one — see
/// [`booth::spawn_booth_effects`]: the body panes are live `<PlayerModel>` widgets in the reference,
/// the round portraits a one-shot cached bake.
#[derive(Component)]
pub(crate) struct PortraitEffects {
    /// The BODY bone whose booth joint the effect model's host seats on.
    pub(crate) bone: u16,
    /// The **M2 attachment id** the effect-bearing model hangs from on the body, for
    /// [`attach_reset`]. Every publisher of this marker is an item's model, so it is always
    /// `Some`; the option shape is the other two markers'.
    pub(crate) attach: Option<u16>,
    /// The host's offset in that bone's joint frame (Bevy axes) — the item's attach point, plus the
    /// glow slot's offset on the item's own model when this is a glow.
    pub(crate) offset: Vec3,
    pub(crate) emitters: Vec<benilla_assets::ModelEmitter>,
}

/// Stamped on a **stand-in subject** — a bodiless entity that exists only to be mirrored, whose
/// [`PortraitPart`] / [`PortraitBillboard`] children are assembled straight from the display cache
/// ([`crate::entities::Creatures::display_mirror`]) instead of from a unit's attach path.
///
/// It is what lets a body booth be pointed at a creature that has **no world object at all** — the
/// stable window's stabled pet, which is a row in the server's character-pet cache and a
/// `CreatureDisplayInfo` id, nothing more. The alternative would have been a second bake path
/// beside [`sync_body_booth`]; a subject entity means the *existing* one runs verbatim, so the
/// summoned-pet case and the stabled case cannot drift apart in framing, lighting, yaw or settle.
///
/// It carries the display id because that is the one thing [`sync_body_booth`] otherwise reads off
/// the unit's [`crate::entities::NetEntity`] — and a stand-in has none, by construction. Nothing
/// else about it is a unit: no guid, no transform of consequence, no visibility, no descriptors.
#[derive(Component)]
pub(crate) struct PortraitStandIn(pub(crate) u32);

/// What a portrait slot currently shows — the booth's live bake, or the ref's 2D stand-in file
/// while a unit's model is still streaming in (`0x525ba0`).
#[derive(Clone, PartialEq)]
pub(crate) enum PortraitSource {
    /// The slot's off-screen render target (the model bake).
    Live(Handle<Image>),
    /// A flat portrait BLP (`Interface\CharacterFrame\TemporaryPortrait-…`), resolved by the UI
    /// extract through the standard sprite path.
    File(String),
}

/// **The GUID-keyed bake cache** — `0xc0ce7c` (decision 1640; report B334).
///
/// `SetPortraitTexture` on a player GUID whose object the client does not hold does **not** go
/// straight to the 2D stand-in: it probes this cache by guid and, on a hit with a live handle,
/// **binds the cached bake** (`0x525c36 call 0x770300`) — the face the member had when you last
/// saw them. Only a miss loads `TemporaryPortrait-{Male|Female}-{Race}`. So a party member who
/// walks out of visibility range keeps their portrait, and a member you have never seen falls to
/// the stand-in: exactly the two halves of the reported behaviour.
///
/// **How a bake gets in here.** A booth renders into its own target image and then sleeps, so that
/// image already *is* a still of the last bake — the retention is a **handover**: at the moment a
/// party booth loses its member, its target moves in here under that member's guid and the booth
/// is given a fresh one ([`sync_portraits`]'s empty arm). The UI keeps sampling the same handle it
/// was sampling a frame earlier, so the frozen face costs no flicker and no copy — one 256²
/// texture per retained member, allocated only at a handover.
///
/// **Two stated deviations from `0xc0ce7c`** (which caches every portrait draw for the whole
/// session and is emptied only by `ClientDestroyGame 0x401ee0`):
/// - only the four **party** slots hand over. They are where the observable lives: a target out of
///   the object manager has already been deselected, and the pet slot takes the reference's own
///   creature leg. A general cache would pay a texture for every unit ever portraited.
/// - it is **capped** at [`Self::CAP`] entries, oldest-first, instead of growing all session. The
///   cap is memory, not policy — and it is not cleared on disconnect either, because a guid's face
///   does not go stale with the socket; the reference's clear is its graphics graph coming down,
///   not the cache expiring.
#[derive(Resource, Default)]
pub(crate) struct PortraitBakes {
    faces: HashMap<u64, Handle<Image>>,
    /// Insertion order, oldest first — the eviction queue for [`Self::CAP`].
    order: Vec<u64>,
}

impl PortraitBakes {
    /// How many members' faces are kept. Four is a full party; the slack covers a roster that
    /// churns (somebody leaves out of range and is re-invited) without unbounding the memory.
    const CAP: usize = 8;

    /// The bake standing for `guid` — the cache probe `0x525ba0` makes before it reaches for the
    /// stand-in.
    fn get(&self, guid: u64) -> Option<Handle<Image>> {
        self.faces.get(&guid).cloned()
    }

    /// Take a booth's target as `guid`'s face. A re-bake replaces the entry (the reference
    /// re-stores the handle at `+0x24` too), and only a genuinely new guid can evict.
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

/// The bridge between the booth and the UI: unit token (`"player"`/`"target"`) → what its portrait
/// region shows. The UI extract pass ([`crate::ui_script`]) reads it for a `SetPortraitTexture`-bound
/// region; the booth writes it (Live ↔ File transitions included).
#[derive(Resource, Default)]
pub(crate) struct PortraitImages(pub(crate) HashMap<String, PortraitSource>);

/// The paper-doll model pane's live input (decision 0208 §5): the bake **yaw** in radians — the
/// ref's `Model:SetRotation` convention (rotate-left *decrements*; the pane's default is `0.61`, a
/// three-quarter view). The character window's feed writes `yaw` each frame (from the rotate
/// buttons / drag); the [`PAPERDOLL_SLOT`] booth spins the model root to match and re-bakes only
/// when it (or the dressed look) changes — never every frame. The bake lands in [`PortraitImages`]
/// under the `"paperdoll"` key, sampled square by the model pane's region.
#[derive(Resource)]
pub(crate) struct PaperDollBooth {
    pub(crate) yaw: f32,
}

impl Default for PaperDollBooth {
    fn default() -> Self {
        Self { yaw: 0.61 }
    }
}

/// The inspect window's model pane input (decision 0631 §4) — the [`PaperDollBooth`] twin, plus
/// the one thing the paper doll never needs: **which unit**. `unit` is the entity
/// [`crate::ui_inspect`] resolved the inspected token to this frame, `None` when nothing is being
/// inspected (or the target isn't streamed), which empties the booth.
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

/// The pet paper doll's model pane input (decision 1057) — the [`InspectBooth`] shape exactly:
/// a yaw the pane's rotate buttons write, and the unit [`crate::ui_pet_doll`] resolved the pet
/// token to this frame (`None` with no pet out, or while its object hasn't streamed — which empties
/// the booth, the same "no pet" the rest of the page shows).
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

/// The stable window's model pane input — the
/// [`PetDollBooth`] shape plus the one thing no other body pane needs: a **subject with no world
/// object**.
///
/// `GetSelectedStablePet()` answers `0` for the summoned pet, `1..=2` for a bought stable slot and
/// `-1` for nothing, and the reference's `SetPetStablePaperdoll` forks on exactly that: a live pet
/// is resolved by GUID and its model taken, while a stabled — or merely dismissed — pet is a row in
/// the server's character-pet cache with nothing but a creature entry, resolved through the
/// creature cache. So this booth carries both, and [`sync_stable_booth`] prefers the live one:
///
/// - `unit` — the summoned pet's entity, when the selection is slot 0 **and** the pet is actually
///   out and streamed. Then the pane is [`PETDOLL_SLOT`]'s case exactly, mirrored from the world
///   body, and [`sync_body_booth`] handles it unchanged.
/// - `display_id` — the selected pet's `CreatureDisplayInfo`, from the creature query
///   ([`crate::names::CreatureRecord::display_id`]). Used whenever `unit` is `None`: every stabled
///   pet, and slot 0 while the pet is dismissed or out of range to summon (the server still sends
///   its row, and the reference still draws it — `0xb72060`, the summoned record's creature entry,
///   is the fall-through of the live branch's own GUID miss).
///
/// Both `None` = nothing selected, or the query has not answered yet, which empties the booth.
///
/// ## No pet-family size ramp here, and that is checked rather than assumed
///
/// Decision 1538 sizes the *select screen's* pet by `CreatureFamily`'s level ramp. **This pane
/// applies no scale at all**, because the reference applies none: `SetPetStablePaperdoll`
/// (`0x4cb870`) writes no scale on either arm; `0x505cb0` only stores the pointer into
/// `[widget+0x3f0]` and clears `[+0x3e0]`/`[+0x3e4]`; and the record→model build it defers to
/// (`0x505a70`) walks `CreatureDisplayInfo` → `CreatureModelData` → the model path and calls
/// `0x4797b0`, the creature **texture-variation** applier — there is no `fmul` of a scale anywhere
/// on the path. The pane is unnormalized by construction anyway ([`framing::body_frame`]: a
/// `<PlayerModel>` renders through the model's own authored camera or a fixed rig, never a bounds
/// fit), so "small model draws small" is already the law here.
///
/// The second reason is internal, and it is the stronger one: the live arm goes through
/// [`sync_body_booth`] like every other body pane, which applies only [`framing::pane_root_scale`].
/// Ramping the display arm alone would make **the same pet change size** the moment it was
/// dismissed — one pane telling two stories.
#[derive(Resource)]
pub(crate) struct StableBooth {
    pub(crate) yaw: f32,
    pub(crate) unit: Option<Entity>,
    pub(crate) display_id: Option<u32>,
}

impl Default for StableBooth {
    fn default() -> Self {
        Self {
            // The reference's own `<PlayerModel>` default facing (UIParent.lua:1422), seeded again
            // by `BenillaPaperDollModel_OnLoad` the moment the window loads.
            yaw: 0.61,
            unit: None,
            display_id: None,
        }
    }
}

/// The key identifying what a booth currently has baked. Any change in the unit's dressed look
/// (gear swap, appearance refresh, different unit) changes it → re-bake.
#[derive(PartialEq)]
struct LookKey {
    /// The (mesh, material) identity of every mirrored draw — [`PortraitPart`],
    /// [`PortraitRider`], [`PortraitBillboard`].
    draws: Vec<(AssetId<Mesh>, AssetId<WowModelMaterial>)>,
    /// One entry per mirrored [`PortraitEffects`]: its seat (bone + offset bits) and how many
    /// emitters it carries. Effects carry no asset handle to key on, and they can arrive **later**
    /// than the meshes they ride with — an item glow resolves asynchronously
    /// ([`crate::entities::item_glow`]: a template round trip, then the effect model's own load) —
    /// so a key blind to them would leave the glow out of the bake until some unrelated edge forced
    /// a re-bake.
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

/// Per-slot booth: its render layer, the model-root entity (children = the baked model's meshes),
/// its render-target image (so the bridge can flip back to `Live` after a `File` stand-in), and the
/// parts key currently baked (`None` = empty booth).
struct Booth {
    layer: RenderLayers,
    root: Entity,
    target: Handle<Image>,
    baked: Option<LookKey>,
    /// **Whose face is standing in this booth** — the guid the current [`Self::baked`] belongs to,
    /// for the handover into [`PortraitBakes`] when the booth loses them (decision 1640). Set by
    /// the party slots alone; `None` on every other booth, which never hands over.
    baked_guid: Option<u64>,
    /// **Body panes only** — what [`sync_body_booth`] last snapshotted, in place of `baked`.
    /// A body pane is a `<PlayerModel>` widget and re-takes its model on the reference's own
    /// four triggers, never on the world moving something ([`SnapKey`]).
    snap: Option<SnapKey>,
    /// The pane was on screen last frame ([`BoothPanes`]) — the edge detector for the widget's
    /// C++ show override (`0x505d00`), which re-duplicates on every hidden→visible transition.
    shown: bool,
    /// How many times that edge has fired. Lives in [`SnapKey::show`].
    show_rev: u32,
    /// Demand-render window (decision 0540): frames [`gate_booth_cameras`] still keeps this
    /// booth's camera active. Armed to [`BOOTH_SETTLE_FRAMES`] by every content edge (bake,
    /// empty, framing/yaw write); 0 with `pending` drained = the camera sleeps and the target
    /// keeps the last render — a still costs nothing per frame.
    wake: u32,
    /// The bake standing in this booth is a **live widget**, not a still: its Stand loops and its
    /// item emitters run ([`booth::spawn_booth_effects`]), so it must re-render every frame — but
    /// only while something is drawing it, which [`BoothPanes`] answers (decision 1069). The
    /// generalization of the glue booth's own always-on rule (`live_scene` below, decision 0540);
    /// only the body panes set it (decision 0822 §4 — the round portraits are a one-shot bake).
    live: bool,
    /// Textures the last bake referenced that were not yet resident: the camera stays awake
    /// until each lands (an `mpq://` image arriving after the bake would otherwise be frozen
    /// OUT of the still forever), then renders one final resident frame.
    pending: Vec<Handle<Image>>,
    /// When the current `pending` hold began (wall secs) — `None` while it is empty. The hold
    /// was designed for a texture in flight, not one that will NEVER land (a failed load leaves
    /// no `Assets<Image>` entry): unbounded, that pinned the camera rendering forever behind a
    /// closed window. [`gate_booth_cameras`] releases the hold at [`PENDING_LANDING_SECS`]
    /// with a warn — the still keeps whatever did land.
    pending_since: Option<f64>,
    /// **This bake has not yet been drawn with every pipeline compiled** — the [`Booth::pending`]
    /// law, one level further down the stack (report B331).
    ///
    /// `pending` asks whether a texture is resident in `Assets<Image>`; that is a *main-world*
    /// question, and it is not the whole of "did this draw appear". Off macOS Bevy builds each
    /// pipeline variant on the async pool, and a batch whose variant is still building is
    /// **skipped** — silently, with no error and no missing asset ([`PipeWatch::compiling`] has
    /// the byte references). A live view redraws it a few frames later and nobody ever sees it.
    /// A one-shot portrait bake does not: [`BOOTH_SETTLE_FRAMES`] elapse, the camera sleeps, and
    /// the still keeps the hole for the rest of the session — which is exactly the symptom (the
    /// hair missing one time, the face the next, at random): hair is the alpha-key
    /// pipeline, the body the opaque one, the eye-glow card the additive one, and which of the
    /// three had landed by the fourth frame is a race. It is invisible on macOS, where the same
    /// compile `block_on`s the render thread instead: the whole class exists on the reporters'
    /// machines and on no screen we can look at. Decision 1621.
    ///
    /// Set by [`wake_booth`]; [`gate_booth_cameras`] holds the camera awake while the cache is
    /// draining and then spends one final rendered frame, exactly as it does for `pending`.
    pipes_settling: bool,
    /// When the current [`Booth::pipes_settling`] hold began (wall secs) — `None` while it is
    /// clear. Bounded like [`Booth::pending_since`] and for the same reason: a session that keeps
    /// meeting new variants (walking a city) keeps the cache non-empty, and an unbounded hold
    /// would pin a 256² camera rendering behind it.
    pipes_since: Option<f64>,
    /// The **destination pane's** aspect this booth's camera is currently framed for
    /// ([`framing::WowPortraitProjection::aspect`], decision 1069) — 1.0 until the UI has drawn the
    /// pane once, then sticky: a hidden window must not re-frame the bake back to square.
    aspect: f32,
    /// A rig stands in this booth ([`booth::BoothRig::rigged`] at the last bake) — the park
    /// gate's "is there anything to park". Cleared with the emptied booth.
    rigged: bool,
    /// The rotate arrows' turn animation ([`Turn`], decision 1559) — body panes only.
    turn: Turn,
    /// The booth scene is parked: its camera is asleep, so [`gate_booth_cameras`] put
    /// [`benilla_world::rig_anim::AnimParked`] on the root — the 0712 evaluator, the pose
    /// composes, the palette writes and the global-sequence writes all skip (the pose HOLDS —
    /// the buffer is state), and the booth-lane emitters freeze under the draw-set law
    /// (`particles::sim` — the reference only ticks an emitter its frame draws). A camera wake
    /// drops the marker before the animation lane runs, so the wake window's first render
    /// already animates. Costs nothing while a window is open; retires the whole idle lane the
    /// rest of the session.
    parked: bool,
}

/// A body pane's **turn animation** state — the half of the reference's `SetRotation` that is not
/// the facing write (decision 1559, director report B313).
///
/// `PlayerModel:SetRotation(angle)` (`0x505bb0`) does two
/// things: it picks a turn-in-place shuffle by direction, queues it and arms a 100 ms expiry, and
/// *then* writes the facing field `SetFacing` writes. Ours only ever did the second — the doll
/// spun on the spot while the reference's steps its feet round, which is what a held arrow looks
/// like there.
///
/// The producer/consumer split is one frame's width and deliberate: [`sync_body_booth`] holds the
/// yaw and sets [`Self::spun`]; [`booth::drive_booth_turn`], which holds the `AnimationPlayer`,
/// spends it. The syncs run before it in the same chain.
#[derive(Default)]
struct Turn {
    /// The facing this booth's root is posed at — the reference's `[+0x39c]`, and the value its
    /// direction test compares the incoming angle against. `None` until the first pose, which is
    /// why a fresh bake does not step (the reference re-snapshots on `RefreshUnit` without
    /// calling `SetRotation`).
    faced: Option<f32>,
    /// A rotation happened this frame, and the `AnimationData` id it arms
    /// ([`booth::turn_shuffle`] picked it — a shuffle, or `Stand` for the equal/NaN case). Set by
    /// the sync, taken by the driver, at most one frame old.
    spun: Option<u16>,
    /// The shuffle currently stepping — its `AnimationData` id and the wall-clock second it
    /// expires at. `None` = the doll is back on Stand.
    shuffle: Option<(u16, f64)>,
    /// The graph node the turn's **primary** is running — the shuffle while one steps, the Stand
    /// variation it settled into otherwise. Kept because every arm rolls a **frequency-weighted
    /// random variation** (`0x7121a0`'s `-1`), so the node playing is not recoverable from the id
    /// alone. `None` before the first arm, where the bake's own Stand is the pose in flight.
    playing: Option<bevy::animation::graph::AnimationNodeIndex>,
    /// The cross-fade out of the pose the last arm replaced ([`Fade`]) — `None` between turns.
    fade: Option<Fade>,
}

impl Turn {
    /// **The bake replaced this booth's `AnimationPlayer`** — drop every node reference the turn was
    /// holding, because they name `ActiveAnimation`s in a player that no longer exists (the bake
    /// `insert`s a fresh one on the same root). Without this a re-bake mid-turn — an equipment
    /// change while the arrow is held, the pane's aspect settling — leaves
    /// [`booth::drive_booth_turn`] naming a node the new player never played, and the next weight
    /// write **starts** it: a phantom shuffle at full weight over the doll's idle.
    ///
    /// [`Self::faced`] survives, and must: the yaw is the bake's own (`sync_body_booth` re-poses
    /// the root to it), so a re-bake is not a rotation and must not step the feet. That is the
    /// reference's `RefreshUnit`, which re-snapshots the unit without ever calling `SetRotation`.
    fn rebaked(&mut self) {
        *self = Turn {
            faced: self.faced,
            ..Default::default()
        };
    }
}

/// The **cross-fade out of the pose an arm replaced** — the reference's per-bone SECONDARY blend
/// slot, seeded by every arm the turn makes (decision 1565, director report B321).
///
/// `0x7121a0`'s sixth argument is `1` at both of the turn's call sites (`0x505c23` the rotation,
/// `0x505c98` the 100 ms expiry). A non-zero there is
/// `0x7125d6`: `rep movsd` copies the live primary track into the secondary sub-record `[blk+0xc4]`
/// — the outgoing clip, **still on its own clock**, not a frozen pose — then writes the window end
/// `[blk+0x100]`, the rate `[blk+0x104] = 1/blendTime` and the amplitude `[blk+0x108] = 1.0f`. The
/// kernel at `0x714880` decays λ = smoothstep across it and blends `out = primary + (secondary −
/// primary)·λ` ([`crate::creature_anim::select::blend_lambda`], the same curve the world lane's
/// key-bone slot already runs — decision 0878).
///
/// **One slot, not a list**, exactly as the client keeps one: an arm inside a running window
/// overwrites `[blk+0xc4]`, and the pose it was fading out is dropped there and then.
#[derive(Clone, Copy)]
struct Fade {
    /// The outgoing graph node. Its clock is left running — the `rep movsd` copies the live
    /// track's window base and rate, so the client fades out of a clip that is still stepping.
    node: bevy::animation::graph::AnimationNodeIndex,
    /// The wall-clock second λ reaches 0 at (`[blk+0x100]`).
    until: f64,
    /// The window's full width — the **incoming** clip's own `M2Sequence.blendTime` (`0x7125f2`),
    /// which is why a release settles so much more slowly than a turn starts: HumanMale authors
    /// 0.25 s on ShuffleLeft/Right and **0.5 s** on Stand (`benilla-extract m2seq`).
    span: f32,
}

/// How many frames a content edge keeps a booth camera rendering ([`Booth::wake`]): covers the
/// command-applied spawn, the billboard re-face's one-frame lag, and GPU upload of fresh meshes.
const BOOTH_SETTLE_FRAMES: u32 = 4;

/// This rig stands on a booth stage, not in the world: its `AnimParked` marker is owned by
/// [`gate_booth_cameras`] alone, and the world-view parker (`creature_anim::lod`) filters it out
/// (decision 1447). One marker, one writer — the same split the doodad draw gate holds for
/// placed doodads (decision 1365). The hazard is structural: a booth stage sits outside every
/// world frustum by construction, so an unfiltered view parker freezes every pane moments after
/// its bake — while this gate, tracking only its own park edges, cannot heal the foreign marker.
/// [`booth::BoothRig::finish`] inserts it beside the `RigPose`; [`booth::clear_booth_rig`]
/// strips it with the rest. Policy, not machinery — which is why it lives here and not in
/// `benilla_world::rig_anim` beside the marker it fences (the 1160 line).
#[derive(bevy::prelude::Component)]
pub(crate) struct StageRig;

/// How long a [`Booth::pending`] texture hold may keep the camera awake (wall secs) before it is
/// declared never-landing and released ([`Booth::pending_since`]). Generous against a real load —
/// an MPQ image lands in well under a second — because a premature release only costs a stale
/// still, while the old unbounded hold cost a forever-rendering camera.
const PENDING_LANDING_SECS: f64 = 10.0;

/// How long a [`Booth::pipes_settling`] hold may keep the camera awake (wall secs) before it is
/// spent anyway ([`Booth::pipes_since`]). The pipeline cache always drains *eventually* — every
/// build settles `Ok` or `Err` — but it is a **process-global** counter, so a session that keeps
/// meeting new variants (walking into a new city, the first cast of every spell) can hold it
/// non-empty for a long stretch. Generous, because the cost of waiting is one 256² camera and the
/// cost of releasing early is the wrong face on the screen for the rest of the session.
const PIPELINE_SETTLING_SECS: f64 = 15.0;

/// `WOW_BOOTH_LOG=1` — is the booth instrument armed? (Read once.)
fn booth_log() -> bool {
    static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOG.get_or_init(|| std::env::var_os("WOW_BOOTH_LOG").is_some())
}

/// `WOW_BOOTH_LOG=1` — one line per BAKE DECISION, which is the half the gate timeline cannot
/// show: the gate says a camera *drew*, never *what it drew*. B106's first-login shot is a
/// portrait wearing no armour beside a fully-equipped character, so the question is exactly "how
/// many parts/riders did the bake see, and did it ever re-bake when the rest arrived".
/// `verb`: `bake` committed · `wait` abandoned (a source material was not resident, 0744) ·
/// `stand-in` the 2D fallback while no parts are attached · `empty` the booth was cleared.
fn log_bake(
    token: &str,
    verb: &str,
    parts: &[&PortraitPart],
    riders: &[&PortraitRider],
    billboards: &[&PortraitBillboard],
    effects: &[&PortraitEffects],
) {
    if booth_log() {
        // The **attach ids** the bake is dressed at, sorted and de-duplicated. Counts alone could
        // not answer the question — *what is hanging on this doll?* — and the
        // answer is one line: a hunter mid-shot read `at=[2,35]` (bow at HandLeft, arrow at
        // HandArrow) where the reference can only ever show `[2]`. It is also the retest readout
        // for the reference's attach reset ([`attach_reset`]): no id in the cut family may appear.
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

/// The framing anchors for a bake — **`None` means "not ready, come back next frame", never
/// "this display has no anchors".**
///
/// `Creatures::display_anchors` returns `None` in exactly two states, both transient: the display
/// is not in the cache yet, or its `parts` are *"not yet built"* (its own comment). Once built it
/// always answers `Some`; a display that genuinely has no authored portrait camera answers `Some`
/// with `camera: None` inside, which the framing heuristics are built to handle.
///
/// Both bake sites used to paper over that with `unwrap_or(PortraitAnchors { .., pivot_height: 0.0,
/// .. })`. That is not a neutral default: the retired body fit floored the head signal at `0.1` for
/// "a hypothetical bounds-less display", so zero anchors aimed the camera at a 0.1-unit-tall
/// subject — the paper doll zoomed all the way in, and a portrait at the wrong size. (1089
/// retired that fit, and zero anchors are no longer a *zoom* — but they are still the wrong camera:
/// an unbuilt display has no `cameras[1]` to read, so the pane would latch the fixed fallback rig
/// aimed at a zero bbox centre.) And it latches: the camera is aimed once per bake, and the parts
/// key cannot change afterwards
/// (handle ids are stable from `load()`, so mesh/model arrival is invisible to it), so the frame
/// stays wrong until some unrelated content edge forces a re-bake.
///
/// A display id of `None` is a different case — there is nothing to wait for — and keeps the
/// bounds-less anchors it always had.
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

/// `WOW_BOOTH_LOG=1` — the resolved framing for a bake: which anchors it had and where the camera
/// ended up. An opaque near-black pane is what a camera parked *inside* the model renders, so
/// "black" and "zoomed to max" may be one symptom; this is the line that tells them apart.
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

/// `WOW_BOOTH_LOG=1` — where each booth's **posed skeleton** actually ended up, logged whenever it
/// moves (quantised to a centimetre, so a still bake prints once).
///
/// [`log_frame`] says where the *camera* went; this says where the *model* went, and the pair is the
/// whole diagnosis of a bake that came back empty. Decision 1577's carrion bird posed its bones up
/// to y = 4.70 with its authored camera sitting at y = 3.03 — a camera aimed correctly at geometry
/// Bevy was culling against a **bind-pose** bound that stopped at 1.19. Either half alone reads as
/// "looks fine"; only the two together say the camera and the model were in the same place and the
/// pixels still were not.
fn log_booth_pose(
    booths: Res<Booths>,
    rigs: Query<&benilla_world::rig_anim::RigPose>,
    // Last-logged extent per booth, in centimetres — `[min xyz, max xyz]`.
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

/// Arm `booth` after a content edge: render the settle window, plus every frame until each twin
/// material's texture is resident. `twins` = the material handles the bake just installed.
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
    // A fresh hold gets a fresh clock ([`Booth::pending_since`]) — the gate stamps it.
    booth.pending_since = None;
    // …and a fresh bake owes a pipeline settle ([`Booth::pipes_settling`]): the variants this
    // bake's materials specialize to are only queued when the render world first sees them, so
    // the gate cannot judge this until the settle window has otherwise drained.
    booth.pipes_settling = true;
    booth.pipes_since = None;
}

#[derive(Resource, Default)]
struct Booths(HashMap<String, Booth>);

/// Where each booth's bake is actually being **sampled on screen this frame**: slot token → the
/// destination region's aspect (width ÷ height). Published by the UI extract for every portrait
/// binding it emits — the *square* `BenillaSetBoothTexture` pane (decision 0208 §5) and, since
/// 1576, the round `SetPortraitTexture` unit portraits beside it. The two consumers below are
/// body-booth-only and unreachable for a round slot (`sync_body_booth` is never called with one,
/// and `live_pane` needs a `live` booth, which only a body pane ever is), so the round rows are
/// inert here and exist for a third reader: [`sync_portraits`]'s `"targettarget"` gate, which asks
/// this resource whether the frame it feeds is on screen at all.
///
/// Two things need it, and neither can be a constant (decision 1069):
///
/// - **Shape.** A booth renders into a square target that the UI stretches to fill the pane's rect
///   (`extract`'s `UvRect::FULL`), so the projection has to run at the *pane's* aspect for the
///   stretch to cancel. Rendering at 1.0 into the dressing room's 316×351 pane made every
///   character 11% too tall (director report, 2026-08-06).
/// - **Liveness.** A body pane's bake *animates* ([`BoothMotion::Loop`] — the reference's
///   `<PlayerModel>` widgets render live, decision 0822 §4), so its camera renders every frame it
///   is on screen — and, now that this resource can say so, **none** when it is not. That last
///   half is also a strict win for the pre-1069 emitter case, which used to render forever behind
///   a closed window.
///
/// One frame stale by construction (the extract runs after the booth syncs), which is why the
/// aspect is latched into [`Booth::aspect`] rather than read fresh: a window that closes must not
/// re-frame the bake it left standing.
#[derive(Resource, Default)]
pub(crate) struct BoothPanes(pub(crate) HashMap<String, f32>);

/// Both directions of the booth↔UI bridge in one system param: the bake a bound region **samples**
/// ([`PortraitImages`]) and the pane geometry the extract **publishes** back ([`BoothPanes`]).
///
/// They travel together because they are the same seam, and because the UI pass had already
/// reached Bevy's 16-parameter ceiling — two more `Res`es there is one too many.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BoothBridge<'w> {
    pub(crate) images: Res<'w, PortraitImages>,
    pub(crate) panes: ResMut<'w, BoothPanes>,
    /// The file panes' half of the same seam (decision 2008): the `ModelPane` arm publishes a
    /// tile request per pane and samples the tile's atlas cell back.
    pub(crate) tiles: ResMut<'w, crate::ui_models::UiModelTiles>,
}

/// The group-facing inputs [`sync_portraits`] needs, in one param: who is in the party
/// ([`crate::ui_party::GroupState`]), the guid→entity index that says which of them are actually
/// STREAMED, the skin-palette table (decision 0720), the pet bar (0990), and the name cache — the
/// only place a member we hold no object for has a race and sex at all (report B315). The index
/// serves the party slots and the pet alike.
///
/// One struct rather than five loose params because `sync_portraits` sits at Bevy's 16-parameter
/// ceiling; it was a bare tuple until the name cache made that tuple's type unreadable. It has
/// since become that system's overflow bag outright — [`BoothPanes`] is not group-facing at all,
/// it rides here because there is no room left in the signature.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PartyBooths<'w, 's> {
    roster: Res<'w, crate::ui_party::GroupState>,
    index: Res<'w, crate::net::GuidIndex>,
    palettes: ResMut<'w, benilla_world::rig_palette::RigPalettes>,
    pet_bar: Res<'w, crate::ui_pet::PetBar>,
    names: Res<'w, crate::names::NameCache>,
    /// What the UI drew last frame ([`BoothPanes`]) — read by the `"targettarget"` slot alone.
    panes: Res<'w, BoothPanes>,
    /// The guid-keyed bake cache + what the handover needs to run (decision 1640): a fresh render
    /// target for the booth that just gave its own away, and that booth camera's
    /// [`RenderTarget`] to re-point at it.
    ///
    /// A query of its own rather than a wider `cams` tuple in [`sync_portraits`]: `aim`'s query
    /// shape is shared by five booth systems across four files, and this is one system's concern.
    /// It takes `RenderTarget` mutably where `cams` takes `Transform`/`Projection`, so the two
    /// never overlap.
    bakes: ResMut<'w, PortraitBakes>,
    images: ResMut<'w, Assets<Image>>,
    targets: Query<'w, 's, (&'static BoothCam, &'static mut RenderTarget)>,
}

/// Tags a booth camera with its slot token, so the model-sync pass can re-frame it per model.
/// (`BoothCam`, not `PortraitCamera` — that name is the authored M2 rig, `benilla_assets::PortraitCamera`.)
/// (`pub(crate)` for the particle census, which reports draws per booth token — decision 0775.)
#[derive(Component)]
pub(crate) struct BoothCam(pub(crate) String);

/// **The display aspect the client's `screencoord` scale is computed from** — its `gxResolution`
/// CVar's `width/height`, gated by the `widescreen` CVar (registered with default `"1"`, so on by
/// default; with it off the client uses `4/3` on any monitor). Its one consumer is
/// [`framing::pane_model_scale`], the model-root renormalize factor a `<PlayerModel>` pane applies
/// to a model with no camera of its own.
///
/// **A stated deviation:** benilla has neither CVar yet, so this reads the primary window's own
/// aspect. That is the same number whenever the client runs at its display's native resolution, and
/// ours always does — we expose no resolution mode list to disagree with. When the graphics CVars
/// land, this resource is the one place to repoint.
///
/// Defaults to `4/3` — the client's own `.data` default pair, and the aspect at which the factor is
/// exactly `1.0`, so a booth that bakes before the window is measured is unscaled rather than wrong.
#[derive(Resource)]
pub(crate) struct GxAspect(pub(crate) f32);

impl Default for GxAspect {
    fn default() -> Self {
        Self(4.0 / 3.0)
    }
}

/// Track the primary window's aspect into [`GxAspect`] (see its deviation note). Cheap enough to run
/// every frame; the booths latch it, so a resize re-scales the standing bake exactly once.
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

/// The body panes' render rate (decision 1444): while a `<PlayerModel>`-family pane (paper
/// doll, dressing room, inspect, pet) is on screen, its booth camera renders every OTHER frame
/// instead of every frame. The pane's per-frame bill is the **second render pass itself** —
/// 1441's trace put ~0.9 ms/frame in graph re-run + drawable pressure, and 1443 measured the
/// open-pane delta at +2.7 with the pass running every frame — so a 512²-class doll at 30 fps
/// is the cheapest half of that back. The pose keeps evaluating at full rate (the park is not
/// touched); only the camera skips, so the held frame is always one the pose just produced.
///
/// `boothHalfRate` is **benilla's own CVar** (the reference has no second view to rate-limit —
/// its doll draws in the main pass, 1069's known rent). **Default ON (half-rate) — restored by
/// 1607.** 1444 shipped it on; 1559 turned it off on the director's look-call (a full-rate doll
/// reads as smoother); the 08-25 weak-GPU perf reports (B329) then measured what that costs — a
/// body-pane booth's off-screen pass every frame, ~1.6 ms at 1600×900 and **7.6 ms at 4K**
/// (an A/B leg), paid on exactly the weak GPUs that reported. The director retested
/// the 30 fps doll and it reads fine, so the cheaper default is back. Full-rate is one
/// `/script SetCVar("boothHalfRate", 0)` away for anyone who wants the smoother cadence.
///
/// Half-rate no longer slows the item effects (why 1559's revert was safe to make, and this one).
/// Booth-lane emitters used to freeze on the camera's own `is_active` bit and so ticked once per
/// *drawn* frame — the draw-set law read onto a skip the reference never makes. They key on
/// [`benilla_world::particles::ViewThrottled`] now: a paced camera's scene is still live, so the
/// sim runs at full rate and only the render is halved (1559). So B312 does not return: only the
/// doll's animation *cadence* in the pane drops to 30 fps, never its item/weapon effects.
///
/// The glue screens (`live_scene`) are exempt: a fullscreen create/select scene at 30 fps is
/// not a pane crop, and those screens have no world behind them to pay for.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PaneRate {
    /// The registered CVar is welded to this in `cvars::tests`, so the shipped default cannot
    /// drift from the registered one.
    pub(crate) half: bool,
}

impl Default for PaneRate {
    /// Half-rate (1607) — see the type's doc. `Default` is hand-written rather than derived so the
    /// shipped default is not silently `false`; the welded `cvars::tests` assert pins it to the
    /// registered CVar either way.
    fn default() -> Self {
        Self { half: true }
    }
}

/// The two **framing inputs** a body booth reads, in one param: the pane geometry the UI extract
/// publishes ([`BoothPanes`]) and the display aspect ([`GxAspect`]).
///
/// Bundled rather than passed side by side because these systems sit on Bevy's 16-parameter ceiling
/// — the same reason [`BoothBridge`] exists on the other side of the seam.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BoothFraming<'w> {
    pub(crate) panes: Res<'w, BoothPanes>,
    pub(crate) gx: Res<'w, GxAspect>,
}

/// Owns the portrait bake pipeline: the [`PortraitImages`] bridge + the per-slot off-screen booths.
pub(crate) struct PortraitPlugin;

/// The body panes' half-rate switch's change callback (1444, 2303): a flag.
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
            // The `"npc"` token's entity is resolved by `ui_session`'s own plugin (it is shared with
            // the interaction face-me, decision 1467) — the booths read whatever it last published.
            // Here the test bake owns the booths when its env is set (the live syncs yield to it),
            // and the paper-doll sync runs last (it shares the camera/booth/image resources, so the
            // chain keeps the access ordered).
            .add_systems(
                Update,
                (
                    // First: the model-changed producers this frame, so every pane below reads
                    // one revision. Its writes are `Commands`, so a bump reaches the panes on the
                    // NEXT frame — which is the reference's own latency, an event queued into
                    // `0x524cd0`'s per-frame drain and consumed by Lua a round-trip later.
                    bump_model_revision,
                    test_bake::sync_test_portraits,
                    sync_portraits,
                    sync_paperdoll,
                    sync_inspect_booth,
                    sync_petdoll_booth,
                    // Before the pane that reads it: the stand-in must exist (and have had a frame
                    // to flush its children) before the booth can mirror it.
                    sync_stable_standin,
                    sync_stable_booth,
                    // **The scene FIRST, then the character standing in it** — the two are one
                    // committed pair (`glue_booth::PendingSwap`), and the booth reads the stage's
                    // spot, facing and light rig off the scene. Running the booth first meant it
                    // always baked against LAST frame's stage, which is one frame of the wrong
                    // character in the new race's scene on every click.
                    glue_booth::sync_glue_scene,
                    glue_booth::sync_glue_booth,
                    // The glue screens' FFX state, off the same look every screen already writes.
                    glue_booth::sync_glue_ffx,
                    // After the scene's framing: the viewport/clear its law decided (1619).
                    glue_booth::pillarbox_glue_scene,
                    // After the scene: the pet's seat and its light are the scene's to publish.
                    glue_booth::sync_glue_pet,
                    dressup::sync_dressup_booth,
                    // After the syncs, which are the only writers of the yaw it spends (1559).
                    booth::drive_booth_turn,
                    // Last: it reads the wake/pending state every sync above may have armed.
                    gate_booth_cameras,
                )
                    .chain(),
            )
            // Re-face each booth's eye-glow cards to its own camera (reads last-propagate joint
            // globals; unordered w.r.t. the syncs — a fresh card just faces forward one frame).
            .add_systems(Update, booth::face_booth_billboards)
            // The booth twin of the world visibility authority's render-alpha write (decision 0807):
            // push each booth part's sampled `MatAnim` onto its tag. Ordered after the shared
            // sampler, the only producer of `MatAnim::current`, so it moves THIS frame's value.
            .add_systems(
                Update,
                booth::push_booth_mat_alpha.after(benilla_world::doodad_anim::sample_mat_anim),
            )
            // The phase-3 preview instrument (`WOW_CREATE_TEST`, decision 0423): inert without the env.
            .add_systems(Update, glue_booth::drive_create_test)
            // `WOW_BOOTH_DUMP=<token>:<path>:<secs>` — photograph a booth's render target to disk
            // (the headless eye on "what is the paperdoll actually showing right now"; the
            // first-login black-pane hunt). Inert without the env.
            .add_systems(Update, test_bake::dump_booth_target)
            // `WOW_BOOTH_LOG=1` — the model half of the framing instrument (see `log_frame`).
            // Inert without the env.
            .add_systems(Update, log_booth_pose);
    }
}

/// A fresh transparent render-target image of `size²`, usable as a camera target and sampled by the
/// UI. Portrait slots pass [`PORTRAIT_SIZE`]; the paper doll passes [`PAPERDOLL_SIZE`].
fn new_target_image(size: u32) -> Image {
    new_target_image_sized(size, size)
}

/// [`new_target_image`] at any size — the UI model tiles' atlas (`crate::ui_models`).
pub(crate) fn new_target_image_sized(width: u32, height: u32) -> Image {
    let mut image = Image::new_fill(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0; 8],
        // **Un-encoded on purpose, and at float precision — the two are separate choices.**
        //
        // Un-encoded (no `…Srgb` label) is measured: with an Srgb label the composited portrait read
        // one net sRGB-decode too dark vs the world render of the same model, because the UI arc
        // composites in GAMMA bytes and does its one decode at the end (`ui_gamma`, decisions
        // 0254/0541) — a booth target that pre-encodes lands a second encode in that chain. So the
        // target holds the booth's own un-encoded values and the UI shader encodes them.
        //
        // FLOAT, not `Rgba8Unorm`, is B126 (decision 0804). Quantizing *un-encoded* values to 8 bits
        // is a precision collapse exactly where the eye is most sensitive: the only display levels
        // reachable below display byte 100 are `srgb(k/255)` = 0, 13, 22, 28, 34, 38, … — ~25 steps
        // where the 8-bit gamma backbuffer this feeds has 100. That is the banding that reads as
        // 16-bit colour on the glue screens' trees and skybox, measured on the
        // histogram of a char-select capture (15 of 16 predicted ladder values hit). The pipeline's
        // *semantics* are unchanged by this — same values, same single encode downstream, just not
        // rounded to a 256-step linear grid on the way through.
        TextureFormat::Rgba16Float,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    image
}

/// The booth **view shape** — the components that set a booth camera's *pipeline key space*:
/// HDR float intermediate + `Tonemapping::None` (the world camera's exact shape — the measured
/// whys sit on the portrait-slot spawn below) at `Msaa::Off`. Defined ONCE and spawned by every
/// booth camera — the portrait slots, the two body panes, the glue booth, and pipe_warm's twin
/// booth — because the warm pass compiles the samples=1 twin of every model pipeline against
/// exactly this shape behind the loading cover (decisions 0938/0958): a booth camera whose shape
/// drifts from the warm booth's is a live pipeline stall on its first bake.
pub(crate) fn booth_view_shape() -> impl Bundle {
    (
        Camera3d::default(),
        bevy::render::view::Hdr,
        Tonemapping::None,
        Msaa::Off,
        // No bevy light ever reaches a booth (its lighting is the material variant,
        // `portrait::light`), so the per-view cluster assignment is dead work here exactly as
        // it is on the world camera (`player::setup`): with the default config every booth view
        // rebuilt its empty cluster grid each frame — 0.2 ms of the raid's main thread for
        // nothing drawn by it.
        bevy::light::cluster::ClusterConfig::None,
    )
}

/// The pipe_warm **twin booth** (decision 0958). The real booths spawn with a
/// `PerspectiveProjection` placeholder and render the menagerie's twins under bevy_pbr's
/// PERSPECTIVE view key — but nearly every real bake installs
/// `Projection::custom(WowPortraitProjection)` ([`frame`]'s authored path; [`body_frame`]
/// always), which is the NONSTANDARD view key: a distinct pipeline per variant, so the whole
/// samples=1 twin space compiled AGAIN, live, on the first target/paperdoll bake after entry.
/// This camera is that missing key space: the booth view shape exactly, with the custom
/// projection class real bakes use. pipe_warm spawns it beside the menagerie and despawns it at
/// drain. A REAL booth's projection is deliberately never stamped for warming — a bake completing
/// during the warm window would keep a misframed image. (The placeholder-Perspective class stays
/// warmed through a real booth: [`frame`]'s camera-less fallback still bakes with it.)
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
                // The warm pass compiles PIPELINES, which the aspect does not key.
                aspect: 1.0,
            }),
            layer.clone(),
        ))
        .id();
    (cam, layer)
}

/// Startup: stand up one booth per slot — its image (registered in [`PortraitImages`]), a model-root
/// entity, and a camera rendering only that slot's layer into the image (transparent, no bloom/MSAA,
/// rendered before the world camera via a negative order).
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
    // The studio-light buffer: same LAYOUT as the shared world light, written once, never touched
    // again (fixed values — no per-frame upload, no render-world system). Sized to the FULL blob
    // (`light_blob_bytes` — header rows + the 0278 point-light table): the model shader declares the
    // whole struct and wgpu validates the bound size against it at every draw. Only the studio header
    // rows are written; wgpu zero-initializes the rest, so `point_count = 0` and no scene point light
    // ever touches a portrait (the studio look is deliberately static).
    let light_buffer = |label: &'static str, blob: benilla_world::lighting::LightBlob| {
        let buffer = blob.create(&device, label);
        blob.write(&queue, &buffer);
        buffer
    };
    booth_light.studio.buffer = Some(light_buffer("wow_portrait_light", studio_light()));
    // The body panes' own light — the reference `<PlayerModel>` widget's (see the fn's doc).
    booth_light.pane.buffer = Some(light_buffer("wow_model_pane_light", model_pane_light()));
    // Booth rigs skin from the palette regions of THESE buffers (decision 0720): register both
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
            // The shared booth view shape — HDR intermediate, no tonemap, no MSAA (its doc has
            // the whys); per-slot here: the order, the backdrop, the glow node, the projection.
            booth_view_shape(),
            Camera {
                // Render the booths first (negative order), so the freshly baked image is ready when
                // the world + UI cameras run. One order per slot to keep them distinct.
                order: -100 + i as isize,
                // The ref bake's opaque near-black backdrop (the world must never show through the
                // circle); the round cut happens at draw time (ui_quad.wgsl's circular mask).
                clear_color: ClearColorConfig::Custom(Color::srgb(0.055, 0.045, 0.04)),
                ..default()
            },
            // The render target is its own component in Bevy 0.18 (Camera `#[require]`s it).
            RenderTarget::Image(image.clone().into()),
            // The gamma lane (0161): booth materials emit gamma bytes like the world's, so the
            // booth needs the same final node — the FFXGlow combine owns the frame's ONE decode.
            // This also keeps the bake at exact world parity (same glow, same transform chain);
            // without it the portrait reads one encode too bright.
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

    // The four **body** booths — the character window's paper doll (decision 0208 §5), the
    // inspect window's pane (decision 0631 §4), the pet paper doll's (decision 1057) and the
    // stable window's (`SetPetStablePaperdoll 0x4cb870`). Same off-screen pipeline as the
    // portrait slots (transparent target, HDR + the FFXGlow node, negative order so the bake is
    // ready before the world/UI cameras), but their own 512² targets, their own layers, and a
    // body-framing projection (aimed per-bake by `sync_body_booth`). Kept a separate spawn from
    // the portrait loop above so these cameras stay byte-for-byte what the director approved.
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
                // **TRANSPARENT** — a `<PlayerModel>` widget has no backdrop of its own (decision
                // 1083). The reference's three model frames declare nothing but the widget; what
                // fills the pane behind the figure is the PAGE's own art, the same
                // `UI-Character-CharacterTab-*` / `UI-Character-General-*` plates our transcription
                // already draws, and the model composites over them. Baking an opaque backdrop into
                // the target painted those plates out — the director's report is exactly that: "our
                // paperdoll is pure black bg instead of having what the ref has, slightly off black
                // and textured" (2026-08-07). The glue booth has composited this way since 0807;
                // the combine passes scene alpha through untouched.
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            // Decode, but NO glow: these two stand in for 1.12 `<PlayerModel>` widgets, which the
            // reference paints in the UI strata — after the WorldFrame's own FFX apply — so they
            // never carry the scene glow (decision 0638; [`benilla_world::ffx_glow::FfxGlow::UI_PANE`]).
            benilla_world::ffx_glow::FfxGlow::UI_PANE,
            // Placeholder — `sync_body_booth` overwrites transform + projection from the unit's
            // bounds on the first bake. A plain perspective is harmless while the model is loading.
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

    // The glue booth (decisions 0423 + 0465): its own slot/layer/target, framed per-bake.
    glue_booth::spawn_glue_booth(&mut commands, &mut images, &mut portraits, &mut booths);
    // The dressing room (decision 1060): a third body pane, tuple-driven like the glue booth but
    // lit and framed like the paper doll.
    dressup::spawn_dressup_booth(&mut commands, &mut images, &mut portraits, &mut booths);
}

/// `true` while the `WOW_PORTRAIT_TEST` debug bake owns the booths (checked once — env vars don't
/// change mid-run).
fn test_mode(cached: &mut Option<bool>) -> bool {
    *cached.get_or_insert_with(|| std::env::var("WOW_PORTRAIT_TEST").is_ok_and(|s| !s.is_empty()))
}

/// benilla's **`UNIT_MODEL_CHANGED`**, for the two producers that are not a change of dress
/// ([`crate::entities::DressKey`] is the rest). One counter per unit; a body pane re-takes its
/// snapshot when it moves ([`SnapKey`]).
///
/// - **The manual sheath ceremony's own keyframe.** `SetSheatheState(…, bInstant = 0)` — which
///   only `ToggleSheath` passes, on all three legs — runs the ceremony, and the clip's `$SHL`/
///   `$SHR` event marks the unit **unconditionally** (`0x611b60` → `0x5ffb10`, the mark at
///   `0x5ffbbe`, a function with one `ret`). So pressing Z moves an open character sheet's doll,
///   one Lua round-trip later. Every *snap* sheath change — the combat auto-draw, the stand-state
///   stow, the descriptor apply — takes `bInstant != 0` and reaches the queue only through the
///   enchant-gated `0x5eed50`, which is why drawing a bow on a mob does **not** move the doll.
/// - **An item's glow instances landing** — ours, not the reference's. Its widget duplicates a
///   model the world had already finished building; our `ItemVisuals` models stream in, and a key
///   blind to their arrival would leave a permanently-glowing weapon glowing nothing in the
///   character window (the case [`LookKey`]'s effects term was added for).
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
        // A glow's root is chained to the WEARER (`spawn_slot` stamps `ParentModel(wearer)`),
        // which is the unit whose model just gained geometry.
        .chain(glows.iter().map(|p| p.0));
    for unit in units {
        if !bumped.insert(unit) {
            continue;
        }
        let next = revs.get(unit).map_or(0, |r| r.0).wrapping_add(1);
        commands.entity(unit).try_insert(ModelRevision(next));
    }
}

/// The attach ids the **sheath lane owns** — the hand points a weapon is drawn into, the forearm a
/// shield takes, and the six sheath points plus the shield's back slot it is stowed at. A draw or
/// a stow moves an item among these, and for two kinds of item makes it vanish outright: a
/// **ranged** weapon renders nothing while stowed (`0x611770` detaches it and never
/// re-attaches), and so does a melee weapon whose `SheatheType` is 0. The worn
/// quiver rides `0x1a` on the same gate.
///
/// [`SnapKey`] is blind to every one of them, which is the whole mechanism: a
/// widget's duplicate must not notice that the world drew a weapon.
fn sheath_lane(attach: Option<u16>) -> bool {
    matches!(attach, Some(0..=2 | 26..=28 | 30..=33))
}

/// **What a body pane re-takes its snapshot on** — the reference's re-`SetUnit` set, as content.
///
/// A `<PlayerModel>` does not mirror the unit: it **duplicates** the unit's `CM2Model` once and
/// renders the copy (`0x5059a0` → `0x707400`), and the copy is dead to everything the world does
/// afterwards — `[dup+0x34]` holds the source only long enough to build, and is Released and
/// nulled inside `0x707400` itself.
/// Ours mirrored the live tree every frame, so a bow drawn in combat walked straight onto the
/// character sheet.
///
/// The copy is re-taken on exactly four things, and each is a field here:
///
/// 1. **The pane becoming visible** ([`Self::show`]). Not a Lua event at all — `0x505d00` is the
///    widget's effective-visible SHOW override (primary vtable index 34) and re-duplicates on
///    every hidden→visible edge; index 33 `0x505ce0` destroys the model on hide. This is why
///    `PaperDollFrame_OnShow` calls no `SetUnit` and the doll still tracks your gear.
/// 2. **A change of dress** ([`Self::dress`], [`Self::stable`]) — `UNIT_MODEL_CHANGED`, whose one
///    fire site image-wide is `0x524df1`. Equipping or unequipping a visible item marks the unit
///    unconditionally (`0x5e2810` → `0x5dee30`); so do a displayId change, a helm/cloak toggle and
///    an enchant. **Nocked ammo does not** (`0x60ba30` reaches no queue site), and neither does
///    entering combat.
/// 3. **An explicit model event** ([`Self::rev`]) — the manual sheath ceremony's own `$SHL`/`$SHR`
///    keyframe, and our late-arriving item glows. See [`ModelRevision`].
/// 4. **A resize** ([`Self::aspect_bits`]) — `DISPLAY_SIZE_CHANGED` → `RefreshUnit()`, the one
///    re-take the FrameXML does register, and the reason the widget re-snapshots its camera (1069).
#[derive(PartialEq, Eq)]
struct SnapKey {
    /// A different body is a different `SetUnit`.
    unit: Entity,
    /// The mirrored geometry **no sheath change can move**: the unit's own body parts — which
    /// carry the armour composite, and the reference shares that texture object by pointer, so a
    /// re-blit is meant to reach an open doll — plus every rider and card outside
    /// [`sheath_lane`]: the helm, the pauldrons, their glows and their emitters.
    stable: Vec<(AssetId<Mesh>, AssetId<WowModelMaterial>)>,
    /// The same cut over the effect seats [`LookKey`] keys on, for the same reason.
    stable_fx: Vec<(u16, [u32; 3], usize)>,
    /// The three weapon slots, resolved **above** the placement gate — the only lane whose sheath
    /// state can decide whether an item exists at all, and so the one that cannot be read off the
    /// mirrored tree ([`crate::entities::DressKey`]).
    dress: Option<crate::entities::DressKey>,
    rev: u32,
    show: u32,
    aspect_bits: u32,
}

impl SnapKey {
    /// Build the key for `unit`'s pane this frame. `dress`/`rev` are the unit's own components
    /// (absent until its equipment first resolves, which is simply another value); `show` and
    /// `aspect` are the booth's.
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
        // Sorted, so the key cannot move for a reason as incidental as traversal order: adding or
        // removing a sheath-lane child re-orders the siblings popped around it.
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

/// The reference's **attach reset** — does `0x47a230` detach a sub-model hanging at this M2
/// attachment id from the duplicate a model widget renders?
///
/// Every widget that shows a live unit builds its model by *duplicating* the unit's own
/// `CM2Model`, attachment tree and all (`0x707400`/`0x70ea00` deep-copy the children at the same
/// attach ids), and then runs this partial reset on the copy — the `<PlayerModel>` paper doll via
/// `0x5059a0 → 0x47a230`, the round unit portrait via `0x525261`.
///
/// It detaches fifteen ids — `0xf` (twice), `0x10`, `0x11`, `0x12`, `0x13`–`0x19`, `0x1d`, `0x22`,
/// `0x23` — and pointedly **keeps** the three hand points `0`/`1`/`2`, the sheath family
/// `0x1a`/`0x1b`/`0x1c`/`0x1e`–`0x21`, and the worn slots (helm `0xb`, shoulders `5`/`6`,
/// cape `0xc`). Read as a family, the cut is *everything transient*: chest blood front/back,
/// breath, the two nameplates, Base, Head, the two spell hands, Special1–3, Chest — and
/// **HandArrow (`0x23`)**, the nocked arrow. Equipment survives; effects do not.
///
/// `0x23` is the one that shows up in a bug report: our booths mirrored the
/// nocked arrow onto the character-window doll, where the reference can never draw one.
fn attach_reset(attach: Option<u16>) -> bool {
    matches!(attach, Some(0xf..=0x19 | 0x1d | 0x22 | 0x23))
}

/// The reference's **hand grip** for a widget's duplicate, from attachment occupancy alone
/// (`0x5059a0` at `505a34`/`505a4d`: `GetAttachment(model, 2)` non-null → `CloseHand(1)`,
/// `GetAttachment(model, 1)` non-null → `CloseHand(0)` — the cleanest expression of the rule and
/// the one benilla should implement).
///
/// Returns [`booth::spawn_booth_model`]'s `[right, left]`. **Occupancy, not sheath state and not
/// combat**: a hand holding anything closes; the shield's forearm point (`0`) closes nothing. The
/// probe runs over the mirrored set *after* [`attach_reset`], exactly as the reference probes the
/// duplicate after `0x47a230`.
fn hand_grip(
    riders: &[&PortraitRider],
    billboards: &[&PortraitBillboard],
    effects: &[&PortraitEffects],
) -> [bool; 2] {
    // A wand's whole model can be a single camera-facing batch and an emitter set, so the probe
    // has to see every mirror lane, not just the meshes.
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

/// The three queries that read a unit's **dressed look** — the attach-spawned [`PortraitPart`] /
/// [`PortraitRider`] descendants a booth mirrors. Bundled as one `SystemParam` so `sync_portraits`
/// and `sync_paperdoll` stay under Bevy's 16-parameter system ceiling, and share the one
/// descendants walk ([`Self::collect`]) instead of open-coding it twice.
#[derive(SystemParam)]
struct DressedLook<'w, 's> {
    children: Query<'w, 's, &'static Children>,
    parts: Query<'w, 's, &'static PortraitPart>,
    riders: Query<'w, 's, &'static PortraitRider>,
    billboards: Query<'w, 's, &'static PortraitBillboard>,
    effects: Query<'w, 's, &'static PortraitEffects>,
    mounts: Query<'w, 's, (), With<crate::entities::mount::MountBody>>,
    /// The two re-snapshot inputs a body pane cannot read off the mirrored tree ([`SnapKey`]).
    /// They ride here because this is already the "what does this unit look like" param, and
    /// because all three body-pane systems were at Bevy's 16-parameter ceiling.
    dress: Query<'w, 's, &'static crate::entities::DressKey>,
    revs: Query<'w, 's, &'static ModelRevision>,
    /// A [`PortraitStandIn`] subject's display id — the one thing a bodiless subject carries that
    /// a world unit reads off its [`crate::entities::NetEntity`] instead. It rides this param for
    /// the same reason `dress`/`revs` do: all four body-pane systems sit at Bevy's 16-parameter
    /// ceiling, and this is already the "what does this subject look like" bundle.
    stand_in: Query<'w, 's, &'static PortraitStandIn>,
}

impl DressedLook<'_, '_> {
    /// Walk `unit`'s descendants once, collecting its part + rider children. All empty while the
    /// unit's model is still loading / cube-fallback (no attach path has spawned the parts yet).
    ///
    /// A mounted unit's MOUNT child (decision 0441) is a second creature under the unit — a
    /// portrait/paper-doll shows the character alone, never the horse (the ref's `Model:SetUnit`
    /// binds the player model, not the mount). Its **parts** (the mount's body meshes) are
    /// skipped by pruning on the [`mount::MountBody`] marker; **riders** stay collected from the
    /// whole tree, because the rider's own helm/shoulder/held riders hang under the rider's
    /// joints, which re-root under the mount's seat anchor INSIDE the mount subtree while
    /// mounted — and a mount model never carries equipment riders of its own. An item's
    /// camera-facing batches and effects hang off those same joints, so they follow the rider rule,
    /// not the part rule; only the CHARACTER's own billboard (the eye-glow) prunes with the body.
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
            // A camera-facing batch prunes by whose model it is ([`PortraitSeat`]): a mount's own
            // glow card goes with the mount's meshes, an item's card rides an attach joint that
            // re-roots INSIDE the mount subtree while mounted and must survive it like a rider.
            if let Ok(b) = self.billboards.get(e) {
                if (!in_mount || b.seat != PortraitSeat::Body) && !attach_reset(b.attach) {
                    billboards.push(b);
                }
            }
            // Never pruned: every publisher of this marker is an ITEM's model (its own emitters, or
            // its `ItemVisuals` glow's), seated on an attach joint — the rider rule. Nothing
            // publishes a mount's own emitters, so there is no mount case to exclude.
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

    /// A stand-in subject's display id — `None` for a real world unit, which carries its own on
    /// its [`crate::entities::NetEntity`].
    fn stand_in_display(&self, unit: Entity) -> Option<u32> {
        self.stand_in.get(unit).ok().map(|s| s.0)
    }

    /// `unit`'s dress and model revision — `None`/`0` before its equipment has first resolved,
    /// which is simply another key value and re-snapshots when it lands.
    fn snapshot_inputs(&self, unit: Entity) -> (Option<crate::entities::DressKey>, u32) {
        (
            self.dress.get(unit).ok().copied(),
            self.revs.get(unit).map_or(0, |r| r.0),
        )
    }
}

/// Each frame: for every slot, mirror the unit's **live dressed look** — its attach-spawned
/// [`PortraitPart`] children — into the booth whenever that look changes (new unit, gear swap,
/// appearance refresh), re-framing the camera from the display's anchors. A live unit whose model
/// hasn't attached yet shows the ref's 2D `TemporaryPortrait` stand-in instead (`0x525ba0`).
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
        // A token can name a unit the object manager does not hold: a party member outside the
        // local area, a pet whose object hasn't streamed yet. The reference does NOT leave that
        // circle empty — a unit not held as a live UNIT object draws the 2D stand-in (`0x525ba0`),
        // and only "model/subsystem not ready" draws the blank (`0x519fcc`). `unseen` carries that
        // art for exactly those tokens; a token that names nobody at all leaves it `None` and the
        // booth empties as before (report B315).
        let mut unseen: Option<String> = None;
        // The guid this slot names, when the slot is one that retains a bake (decision 1640).
        let mut occupant: Option<u64> = None;
        let unit: Option<Entity> = match token {
            "player" => self_q.single().ok(),
            "target" => selection.target,
            // The target's own target (decision 1576) — one hop off `UNIT_FIELD_TARGET`, the same
            // read the `"targettarget"` unit snapshot makes.
            //
            // **Gated on the UI actually drawing this slot**, which no other slot here is, and
            // which this one needs. The other eight are gated by their own resolution: no pet, no
            // party member, no interact NPC means no unit and no work. This one resolves for as
            // long as anything you have selected is fighting anything — while the frame that
            // draws it ships HIDDEN (1.12's `SHOW_TARGET_OF_TARGET` default is `"0"`), so
            // ungated it would pay a descendants walk every frame and a re-bake on every mob's
            // target switch for a circle nobody is looking at. One frame stale by construction
            // (the extract runs after this), which costs the first bake a frame and nothing else.
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
            // The pet, off the bar's cached guid — the same word its unit token and `UNIT_PET`
            // read (`crate::ui_pet::feed_pet_unit`).
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
            // The NPC an interaction window is bound to (gossip / quest / merchant / trainer),
            // resolved to its live entity by `feed_interact_npc` — the same bake path as "target".
            "npc" => interact_npc.0,
            // A party member's slot bakes only while the member is streamed (in range). Out of
            // range there is no model to pose — and no descriptor either, so the stand-in's
            // race/sex come from the name cache's `SMSG_NAME_QUERY_RESPONSE` triple, warmed for
            // every roster entry by `ui_party::net::list`.
            tok => {
                let member = tok
                    .strip_prefix("party")
                    .and_then(|n| n.parse::<usize>().ok())
                    .and_then(|n| party.roster.party_slots().nth(n - 1));
                // Whose face this slot is showing — the key the bake handover files it under,
                // and the key the absent-unit arm probes the cache with (decision 1640).
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
            // ── The bake handover (decision 1640, report B334) ─────────────────────────────
            //
            // The booth is about to be emptied, and its target is a *still of the face that was
            // standing in it* — so before the clear renders over it, that image becomes this
            // member's entry in the guid cache and the booth is handed a fresh one. The UI is
            // left sampling the very handle it sampled last frame, which now holds the frozen
            // bake: the reference's "bind the cached bake" (`0x525c36`), reached without a copy
            // and without a flicker.
            //
            // Keyed on whose bake is STANDING, not on who the slot names now: a slot whose
            // occupant changed still owes the departed member their face. Only a **settled**
            // bake is worth keeping — an unfinished one (camera still awake, a texture still in
            // flight, pipelines still compiling) would freeze a half-rendered face forever, and
            // the stand-in is the honest answer for it.
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
            // No unit: empty the booth (the frame itself is hidden on UnitExists false; the dark
            // disc behind it never shows).
            if booth.baked.is_some() {
                commands.entity(booth.root).despawn_related::<Children>();
                booth.baked = None;
                // Render the emptied stage (decision 0540): the target must hold the cleared
                // backdrop, not the departed unit's face, before the camera sleeps.
                booth.wake = BOOTH_SETTLE_FRAMES;
                booth.pending.clear();
                // The despawn reaped meshes and anchors; the rig state on the ROOT (pose buffer,
                // player, palette slot) needs its own strip or it evaluates behind the emptied
                // stage for as long as the booth stands empty ([`booth::clear_booth_rig`]).
                clear_booth_rig(&mut commands, booth.root);
                booth.rigged = false;
                booth.parked = false;
            }
            // A unit we hold no object for takes the **cached bake if we have ever baked them**
            // and the ref's 2D stand-in otherwise (`0x525ba0`: hit → bind, miss → the
            // TemporaryPortrait file); only a token that names nobody falls to the emptied
            // render target.
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
        // The unit's dressed look: its attach-spawned part children (geosets filtered, composited
        // materials) + its bone riders (helm/shoulders/held — grandchildren under joint entities)
        // + the camera-facing batches and effect models it wears, one descendants walk. Parts empty
        // while the model is still loading / cube-fallback.
        let (parts, riders, billboards, effects) = look.collect(unit);
        if parts.is_empty() {
            // Model not attached yet → the ref's own 2D stand-in (`0x525ba0`): sex/race for a
            // player body, the Monster art for a creature (our pick — the ref's `-Pet` file belongs
            // to its pet delegate). Keeps the booth's last bake around; only the bridge flips.
            let file = temporary_portrait(ent_q.get(unit).ok(), stores_q.get(unit).ok());
            let src = PortraitSource::File(file);
            if portraits.0.get(token) != Some(&src) {
                portraits.0.insert(token.to_string(), src);
            }
            continue;
        }
        let key = LookKey::build(&parts, &riders, &billboards, &effects);
        // **A changed occupant is a re-bake even at an identical look** (decision 1640). The key
        // is built from mesh/material handles, so two party members in the same race, sex and
        // gear share it — and without this term the booth would keep standing A's bake while
        // `baked_guid` was quietly re-filed to B, so the handover would put that face in the
        // cache under the wrong guid and B would never get one of their own. The bake itself is
        // per-unit; the key alone cannot say so.
        if booth.baked.as_ref() != Some(&key) || booth.baked_guid != occupant {
            let display_id = ent_q.get(unit).ok().and_then(|n| n.display_id);
            // The look changed — re-bake: studio-lit twins of the exact dressed materials, posed
            // at Stand on the booth's own throwaway skeleton (the ref bake — riders ride their
            // bone's joint, exactly like the world instance).
            // Resolve the framing anchors FIRST — before anything is despawned or spawned. A
            // `None` here means the display is still loading (see `booth_anchors`), and a bake
            // committed now would aim the camera at fabricated zero bounds and never re-aim.
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
                    // `None` — the same known gap as the glue preview's (decision 0807): a
                    // mirrored `PortraitPart` doesn't carry the batch's alpha loops.
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
            // The camera-facing batches — the eye-glow on its eye bone, an item's gem/halo at its
            // attach point — seated on their bone's booth joint and camera-faced by the booth (relit
            // onto the studio buffer like everything else; harmless where the batch is fullbright).
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
            // A source material was not resident, so at least one "twin" above is the WORLD
            // material — bound to the world light buffer, which this booth's whole point is to
            // avoid ("would render a night portrait pitch black", `light.rs`). Abandon the bake:
            // `booth.baked` stays untouched, so the parts-key compare re-fires next frame and we
            // retry once the material lands. The previous bake stays on screen meanwhile.
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
                // Open hands, and NOT because "a still portrait sheaths its weapons" — it does
                // not; it mirrors whatever the body holds, same as a pane. The reference's
                // portrait bake (`0x524f60`) duplicates and runs the attach reset `0x47a230`, and
                // that is *all*: unlike `0x5059a0` it never probes the hand points or calls
                // `0x479660`, and the duplicate starts from a fresh `0x70dd70` init that carries
                // no armed anim blocks across. So the reference's own portrait holds a weapon in
                // an open hand — rarely in frame on a bust, and its rule all the same.
                [false, false],
                &booth_billboards,
                BoothInstance::default(),
            );
            // Even a Frozen still re-evaluates its paused pose every frame — the park matters
            // MOST here, since a portrait's camera sleeps for the whole session outside its
            // bake window.
            booth.rigged = booth_rig.rigged();
            booth_rig.finish(&mut commands);
            booth.parked = false;
            // **No emitters here, and that is the reference's own answer** (decision 0822): the
            // round portrait is a ONE-SHOT bake — a fresh M2 scene + instance, one `0x707680` draw,
            // the texture cached by GUID/displayId and returned with *no re-render* on a hit, nothing
            // persisting between bakes (`0x524f60`). A particle
            // emitter contributes nothing to a single frame of a freshly-born pool, so the ref's
            // portrait shows none — and a booth that spawned them would either freeze a cloud
            // mid-birth into the still or have to render forever for a 256² face. The batches above
            // *are* geometry, so they draw in that one frame and belong here. The body panes are a
            // different widget with a different law — see [`sync_body_booth`].
            //
            // Frame through the display's authored portrait camera (heuristic anchors for the
            // camera-less few), resolved above before anything was torn down.
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

/// Each frame: mirror the **player's** dressed look into the paper-doll booth — the SAME
/// [`PortraitPart`]/[`PortraitRider`] children the `"player"` portrait slot mirrors, so a gear /
/// appearance change flips the parts key and re-bakes the full-body pane exactly as it re-bakes the
/// face. Differences from [`sync_portraits`]: only the self player feeds it, the framing is
/// full-body ([`body_frame`] from the model's bounds, never the authored bust camera — decision
/// 0208 §5), and the model root spins to the pane's [`PaperDollBooth::yaw`] (the ref's
/// `Model:SetRotation`).
///
/// **What re-bakes.** A [`SnapKey`] change respawns the posed instance and re-aims the (yaw-
/// independent) camera; a bare yaw change only re-rotates the root — neither happens on an unchanged
/// frame. The bake stands ready whether or not the window is open, but the 512² *pass* only runs
/// while the pane is being drawn ([`BoothPanes`], decision 1069).
///
/// The key is **not** the mirrored geometry. A `<PlayerModel>` duplicates the unit's model once and
/// renders a copy the world can no longer reach, and it re-takes that copy on four things — the
/// pane showing, a change of dress, an explicit model event, a resize. Mirroring live put a bow
/// drawn in combat straight onto the character sheet; [`SnapKey`] carries the whole
/// law and its byte provenance.
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
        return; // the test bake owns the booths (it drives the paper doll too, see `bake_test`)
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

/// The inspect window's model pane (decision 0631 §4) — the paper doll's exact twin, pointed at
/// whichever unit [`crate::ui_inspect`] resolved this frame instead of at the self player.
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

/// The pet paper doll's model pane (decision 1057) — the inspect pane's exact twin, pointed at the
/// pet [`crate::ui_pet_doll`] resolved this frame.
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

/// The stable pane's **stand-in subject** — the bodiless [`PortraitStandIn`] entity that stands
/// for a pet with no world object (module note on [`StableBooth`]).
///
/// Keyed on the **display**, not on the selection: two stabled pets that share a display id are the
/// same model standing at the same facing, so moving the selection between them is not a re-bake —
/// and the reference is no different, since its cached branch resolves a creature record and the
/// two rows resolve to the same one. A *changed* display respawns the entity rather than refilling
/// it, so the bake's own `SnapKey` (keyed on the subject entity) cannot carry across.
#[derive(Resource, Default)]
struct StableStandIn {
    /// The stand-in, while one exists.
    entity: Option<Entity>,
    /// The display it was spawned for.
    display: Option<u32>,
    /// Its mirror children are up. `false` while the display's model is still building, which
    /// simply retries next frame — the glue pet's assembly discipline.
    built: bool,
}

/// Keep [`StableStandIn`] pointing at the selected stabled pet: spawn one when the pane wants a
/// display it has no world body for, fill its mirror children the moment that display's model
/// finishes building, and despawn it the instant the want changes (a different pet, a live pet to
/// mirror instead, or nothing selected).
fn sync_stable_standin(
    mut commands: Commands,
    stable: Res<StableBooth>,
    creatures: Option<Res<Creatures>>,
    mut state: ResMut<StableStandIn>,
) {
    // **The live pet wins.** With a world body to mirror there is nothing for a stand-in to do, and
    // the reference agrees: `SetPetStablePaperdoll` reaches the creature cache only after the
    // live-pet-by-GUID resolve has failed (the fall-through at `0x4cb9bc`).
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
                    // Hidden, at the origin: nothing under a stand-in is ever drawn in the world —
                    // the booth re-spawns its own copies of these meshes on its own layer, which is
                    // what it does for a real unit too. The pair is here so the transform and
                    // visibility propagation walk a well-formed tree rather than a rootless one.
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
    // `None` while the display's model is still loading (`update_display_models` asked for it on
    // this same want) — leave `built` false and retry next frame.
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

/// The stable window's model pane — the pet `GetSelectedStablePet()` names.
///
/// The **whole fork is upstream of here**: [`crate::ui_stable`] resolves the selection into either
/// a live pet entity or a bare display id, and [`sync_stable_standin`] turns the latter into a
/// subject entity. So this is [`sync_petdoll_booth`] with one `or` — which is the point of the
/// stand-in: the summoned pet and the stabled pet reach the same bake through the same code, and so
/// cannot drift apart in framing, lighting, animation or settle.
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

/// Bake `unit`'s full-body dressed look into the `slot` booth at `yaw` — the shared body of all
/// four body booths (decision 0208 §5 for the paper doll, 0631 §4 for inspect, 1057 for the pet
/// doll, `0x4cb870` for the stable pane). `unit` is `None` when there is nothing to show, which
/// empties the booth.
///
/// `unit` is a **subject**, not necessarily a world unit: a [`PortraitStandIn`] mirrors the same
/// way and carries its own display id, which is how the stable pane draws a pet that has no object
/// anywhere ([`StableBooth`]).
fn sync_body_booth(
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    slot: &str,
    unit: Option<Entity>,
    yaw: f32,
    last_pose: &mut Option<(f32, f32)>,
    // The primary window's aspect ([`GxAspect`]) — only the model-root scale reads it.
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
    // Latch the pane's aspect while it is on screen (decision 1069). Sticky: a closed window
    // publishes nothing, and re-framing the standing bake back to square on the way out would be a
    // visible pop on the way back in.
    let aspect = panes.0.get(slot).copied().unwrap_or(booth.aspect);
    // **The show edge** — the widget's own re-take (`0x505d00`, the effective-visible SHOW
    // override at primary vtable index 34, which re-duplicates the unit's model on every
    // hidden→visible transition; index 33 destroys it on hide). `BoothPanes` publishes a slot
    // exactly while its pane is being drawn, so its rising edge is that transition.
    //
    // We re-take rather than destroy-and-rebuild: the two are indistinguishable on screen (nothing
    // is drawn in between) and keeping the bake avoids the re-frame pop 1069's aspect latch exists
    // to prevent.
    let on_screen = panes.0.contains_key(slot);
    if on_screen && !booth.shown {
        booth.show_rev = booth.show_rev.wrapping_add(1);
    }
    booth.shown = on_screen;
    // There is no 2D stand-in for a body pane — the bridge always points at the live target (an
    // empty booth just renders the dark backdrop until the unit's model attaches).
    let live = PortraitSource::Live(booth.target.clone());
    if portraits.0.get(slot) != Some(&live) {
        portraits.0.insert(slot.to_string(), live);
    }
    // The unit's dressed look — the same mirrored descendants the portrait slots collect. Empty
    // while there's no unit, or its model hasn't attached yet.
    let (parts, riders, billboards, effects) = match unit {
        Some(unit) => look.collect(unit),
        None => (Vec::new(), Vec::new(), Vec::new(), Vec::new()),
    };
    if parts.is_empty() {
        // No unit / model not attached → empty the booth and forget the applied yaw (so it
        // re-applies on the next bake).
        if booth.snap.is_some() {
            commands.entity(booth.root).despawn_related::<Children>();
            booth.snap = None;
            *last_pose = None;
            // Render the emptied stage before sleeping (decision 0540) — and the emptied stage has
            // no emitters left, so the pane stops being live.
            booth.wake = BOOTH_SETTLE_FRAMES;
            booth.live = false;
            booth.pending.clear();
            // The despawn reaped meshes and anchors; the rig state on the ROOT needs its own
            // strip ([`booth::clear_booth_rig`]).
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
    // The display the framing, the rig and the model-root scale all key on. A world unit carries
    // it on its wire record; a bodiless [`PortraitStandIn`] carries it on itself, and has no wire
    // record at all — which is the whole of what makes a stabled pet drawable.
    let display_id = ent_q
        .get(unit)
        .ok()
        .and_then(|n| n.display_id)
        .or_else(|| look.stand_in_display(unit));
    // Anchors first, before any teardown — a still-loading display must not be framed from
    // fabricated zero bounds (see the portrait site, and `booth_anchors`). Resolved out here rather
    // than inside the bake because the model-root scale below needs them on an otherwise-idle frame
    // too (a window resize moves the scale without touching the bake).
    let anchors_now = booth_anchors(creatures, display_id);
    // The model-root scale, taken while the anchors are still in hand (the bake below consumes
    // them). `1.0` until they resolve — an unscaled root is the harmless default, and the 4:3
    // factor is 1.0 anyway.
    let model_scale = anchors_now
        .as_ref()
        .map_or(1.0, |a| framing::pane_root_scale(a, display_aspect));
    // A changed pane aspect re-runs the same path: the camera's projection depends on it, and it
    // only ever moves once — the first frame the window is drawn.
    // **The snapshot compare.** This used to be `LookKey`, the
    // mirrored geometry, which moved the instant the world drew a weapon. It is now the
    // reference's own re-`SetUnit` set, and a draw is not in it.
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
                // `None` — the same known gap as the glue preview's (decision 0807).
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
        // The camera-facing batches — the eye-glow on its eye bone, a wand's gem or the held torch's
        // halo at its attach point — seated on their bone's booth joint and camera-faced by the booth
        // (relit onto the pane buffer like everything else; harmless where the batch is fullbright).
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
        // The worn items' effects (decision 0822) — an equipped item's own emitters (0813) and a
        // held weapon's `ItemVisuals` glow (0805). Collected BEFORE the teardown for the
        // same reason as everything else here; spawned after the model, which is what hands us the
        // joints they seat on.
        let booth_effects: Vec<BoothEffects> = effects
            .iter()
            .map(|fx| BoothEffects {
                bone: fx.bone,
                offset: fx.offset,
                emitters: fx.emitters.clone(),
            })
            .collect();
        // Same law as the portrait bake: never latch a world-lane material into the pane. Leave
        // `booth.snap` alone and retry next frame (see the portrait site for the full note).
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
            // The pane ANIMATES: Stand loops and the global-sequence bones run, which is what the
            // reference's `<PlayerModel>` widget does (decision 0822 §4 read it as live-rendering
            // and left the pose as a look call; the director made that call — decision 1069).
            BoothMotion::Loop,
            // The grip is the reference's own probe of the duplicate's hand attachment nodes
            // ([`hand_grip`]) — NOT a constant. The `[false, false]` this replaced rested on
            // "the pane sheaths its weapons", which was never true of this lane: the pane mirrors
            // whatever the body is holding, so a drawn weapon arrived in an open hand.
            hand_grip(&riders, &billboards, &effects),
            &booth_billboards,
            BoothInstance::default(),
        );
        // The item effects go up on the posed skeleton's anchors — the body pane is the lane that
        // gets them (the reference's `<PlayerModel>` widget renders live; the round portraits are
        // a one-shot cached bake — [`spawn_booth_effects`]).
        spawn_booth_effects(
            commands,
            &mut booth_rig,
            &booth.layer,
            booth_light.pane.buffer.as_ref(),
            &booth_effects,
            BoothInstance::default(),
        );
        // The turn's node bookkeeping named the player this bake just replaced.
        booth.turn.rebaked();
        // So the whole bake is live, emitters or not — `wake` can't gate a looping animation.
        // `gate_booth_cameras` renders it every frame its pane is on screen, and none when it isn't.
        booth.live = true;
        // A fresh bake is animated by construction; the park state is the new rig's.
        booth.rigged = booth_rig.rigged();
        booth_rig.finish(commands);
        booth.parked = false;
        // Body framing from the display's bounds — the full standing figure, feet-to-crown, at the
        // destination pane's aspect. Resolved before the teardown above; see the portrait site for
        // why it cannot be faked.
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
    // The model root: **yaw → rotation, plus the pane's model scale** — the widget's own
    // `T(pos)·R(facing)·S(s)` with `pos` at the origin (the ref's `Model:SetRotation` writes the
    // facing, and a spin about WoW +Z-up conjugates to a spin about Bevy +Y-up).
    //
    // `s` is [`framing::pane_root_scale`]: `1.0` for a model with its own camera and the
    // display-aspect renormalize factor for one without. It rides the same latch as the yaw because
    // it moves for the same reason — a window resize — and the reference re-snapshots on exactly
    // that (`DISPLAY_SIZE_CHANGED → RefreshUnit()`, which both panes register). Cheap enough to
    // re-derive here rather than key a whole re-bake on: the CAMERA does not depend on the display
    // aspect, only the root does.
    //
    // The **turn animation** rides the facing write, exactly as it does in the reference
    // (`0x505bb0` picks the shuffle, arms its expiry, and only then stores the angle — 1559).
    // Keyed on the yaw alone, not on this block's condition: a re-bake and a window resize both
    // land here without the model having turned, and neither steps its feet in the reference.
    // The comparison runs on the **Lua-facing scalar**, which is the very value the reference
    // passes to `SetRotation`, so its own `current > angle` branch transfers with no sign work.
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
        // A spin is a content edge too (decision 0540): render the new pose, then sleep.
        booth.wake = booth.wake.max(BOOTH_SETTLE_FRAMES);
    }
}

/// What one frame owes a booth's **pipeline settle** ([`Booth::pipes_settling`]) — the decision
/// alone, so the law is testable without standing a render world up around it.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
enum PipeSettle {
    /// The render world is still building variants, so it is still dropping draws: keep the
    /// camera awake rather than freeze a still with a hole in it.
    Hold,
    /// Drained. Spend the settle on one final rendered frame — the same "one more frame" a
    /// landed [`Booth::pending`] texture buys, and for the same reason.
    Spend,
    /// Nothing owed *yet*: the cache reads idle, but this bake's own [`BOOTH_SETTLE_FRAMES`]
    /// window has not drained, so the reading cannot be trusted about it.
    Idle,
    /// [`PIPELINE_SETTLING_SECS`] ran out — spend it anyway, loudly.
    Expired,
}

/// The settle's one rule, and the ±1-frame race it exists to survive.
///
/// A bake's pipeline variants are queued by the **render** world, the frame after the main world
/// spawned its meshes, and `PipeWatch`'s counters cross back a frame later still. So an idle
/// reading taken at the moment of the bake is a reading of the cache *before* this bake existed —
/// which is why `Spend` waits for `wake_drained`: by the time [`BOOTH_SETTLE_FRAMES`] have gone
/// by, whatever this bake queued is counted.
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

/// The demand-render gate (decision 0540): each booth camera is active only while its booth has
/// something new to show — [`Booth::wake`] frames after a content edge, or a bake texture still
/// in flight ([`Booth::pending`]) — except the booths whose content is **live**, which render
/// continuously: the glue booth, whose whole scene is animated (looping sequences, global-sequence
/// bones, particle emitters) while a glue screen shows, and a **body pane**, whose bake is a live
/// `<PlayerModel>` widget ([`Booth::live`], decisions 0822 §4 + 1069). A sleeping camera
/// skips its whole pass (clear + model + FFXGlow chain); its target keeps the last render — exactly
/// right for a still (the 0105 bake, frozen at Stand).
///
/// A live pane renders **only while it is on screen** ([`BoothPanes`], decision 1069): a character
/// window nobody opened costs nothing, which is the follow-up [`sync_paperdoll`] named and 0822's
/// unconditional `live` never had.
/// With `WOW_PORTRAIT_TEST` set the gate stands down (the eyeball harness wants live cameras).
/// The pipeline warm pass is demand too (decision 0938): its menagerie duplicates rigs onto a
/// booth layer so the booths' `Msaa::Off` pipeline twins compile behind the entry cover — which
/// only works if the booth cameras render during the warm window.
fn gate_booth_cameras(
    mut commands: Commands,
    mut booths: ResMut<Booths>,
    preview: Res<GluePreview>,
    panes: Res<BoothPanes>,
    images: Res<Assets<Image>>,
    warm: Res<crate::pipe_warm::WarmPass>,
    cover: Res<crate::loading_screen::EntryCover>,
    // The pipeline settle's input ([`Booth::pipes_settling`]) — is the render world still
    // building variants, i.e. is it still dropping draws on the floor.
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
    // `WOW_BOOTH_LOG` only: the marker's REAL state beside this gate's `booth.parked`
    // bookkeeping. The two can desync exactly one way — a foreign writer — and that desync is
    // invisible in every capture (a woken rig snaps to the absolute clock), so it must be
    // loggable (decision 1447: the world parker froze every pane and no dump could show it).
    markers: Query<(), With<benilla_world::rig_anim::AnimParked>>,
    mut env_cache: Local<Option<bool>>,
) {
    let test = test_mode(&mut env_cache);
    // `satisfied()` is false exactly while the covered warm window runs (the loading screen
    // holds on it), so this term costs nothing outside that window.
    //
    // **…and not before the cover has reached the glass.** The warm window opens on the
    // state-flip frame, so without the second term all fifteen booth cameras woke on the one
    // frame that owes the glass a loading screen — fifteen extra render passes in front of the
    // present the player is waiting for, and nothing to pick up yet (the menagerie has not
    // spawned; that is what these bakes are awake FOR). 0962's rule, held once in
    // [`crate::loading_screen::EntryCover`].
    let warming = !warm.satisfied() && cover.presented();
    for (cam_entity, BoothCam(token), mut cam, was_paced) in &mut cams {
        let Some(booth) = booths.0.get_mut(token.as_str()) else {
            continue;
        };
        // A pending texture landing this frame still needs one rendered frame to reach the still.
        let had_pending = !booth.pending.is_empty();
        booth.pending.retain(|h| !images.contains(h));
        if had_pending && booth.pending.is_empty() {
            booth.wake = booth.wake.max(1);
        }
        // Bound the hold (the `pending_since` field's doc): a texture that will never land must
        // not keep this camera rendering for the rest of the session.
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
        // The **pipeline settle** ([`Booth::pipes_settling`]), the pending hold's twin one level
        // down: a batch whose pipeline variant is still being built draws NOTHING and says
        // nothing about it, so a still committed inside a compile burst keeps the hole. Judged
        // only once the settle window has otherwise drained — the variants this bake needs are
        // queued by the RENDER world, and these counters cross back ±1 frame, so an idle reading
        // taken the frame of the bake is a reading of the frame before it.
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
        // A live bake renders every frame — but only while the UI is actually drawing its pane.
        // (The glue screens sample their booth outside the FrameXML extract, so they publish no
        // pane and stay on `live_scene`.)
        let live_pane = booth.live && panes.0.contains_key(token.as_str());
        let active = test
            || warming
            || live_scene
            || live_pane
            || booth.wake > 0
            || !booth.pending.is_empty()
            || settling;
        // Half-rate (decision 1444, [`PaneRate`]): when the live pane is the ONLY thing keeping
        // this camera rendering — no wake window settling a fresh bake, no pending texture hold,
        // no fullscreen glue scene — skip every other frame. `active` stays the LOGICAL state:
        // the park bookkeeping below keys off it (the pose keeps evaluating; only the render
        // skips), and the wake counter keeps draining per real frame.
        //
        // `paced` is the REGIME — is this camera being rate-limited at all — and `throttled` is
        // this frame's parity within it. The regime is what the rest of the engine needs to know:
        // a camera whose render we skipped is not a camera whose scene stopped, and the emitter
        // lane keys on exactly that distinction ([`benilla_world::particles::ViewThrottled`],
        // decision 1559 — B312's half-speed item effects were it reading our skip as a cull).
        // Marking the regime rather than the parity also keeps the archetype still: the marker
        // moves when a pane opens or closes, not twice every frame.
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
        // `WOW_BOOTH_LOG=1`: the gate's timeline — every activity flip and every armed frame,
        // wall-stamped (the first-login black-pane hunt).
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
        // Park/unpark the standing scene with its camera (director report 2026-08-19: the doll
        // bake animated at full cost from the LOGIN bake to quit, window opened or not). The
        // park is one `AnimParked` marker on the root (decision 1443): the 0712 evaluator, the
        // pose composes, the palette writes and the global-sequence writes all check it — the
        // pose HOLDS, the buffer is state. Booth-lane emitters freeze on the camera bit itself
        // (`particles::sim`). This system runs before the PostUpdate animation lane, so an
        // unpark's marker drop is seen the same frame — the wake window's first render already
        // animates (0739's wake law, same as every world rig).
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

/// The stand-in art's folder + stem, shared by the three arms below.
const TEMPORARY_PORTRAIT: &str = "Interface\\CharacterFrame\\TemporaryPortrait";

/// The ref's 2D portrait stand-in for a not-yet-renderable unit (`0x525ba0`):
/// `TemporaryPortrait-{Male|Female}-{Race}` for a player body, `-Monster` otherwise.
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

/// The player half of [`temporary_portrait`], taking the two bytes rather than a descriptor — a
/// party member outside the local area HAS no descriptor, and reaches the same art through the
/// name cache's `SMSG_NAME_QUERY_RESPONSE` triple instead (report B315).
///
/// An unknown race falls to the plain `TemporaryPortrait.blp`, which is the ref's own behaviour
/// when its `%s-%s` format has no race string to substitute; an unknown sex reads Male, the
/// zero-byte default.
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

/// The creature stand-in — the Monster art rather than the ref's `-Pet` file (our pick; see
/// [`sync_portraits`]'s not-attached-yet arm).
fn creature_temporary_portrait() -> String {
    format!("{TEMPORARY_PORTRAIT}-Monster.blp")
}

/// Set the named slot's camera to the rig `frame` built — transform AND projection (the authored
/// camera brings its own fov/near/far, so the projection is per-bake, not booth-fixed).
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

    /// **The cache keeps faces, not slots** (decision 1640, report B334): a re-bake of somebody
    /// already in it replaces their entry rather than queueing a second one, and only genuinely
    /// new members can push the oldest out. Get this wrong — count a replace as an insert — and a
    /// party member whose gear changes a few times evicts the other three.
    #[test]
    fn the_bake_cache_replaces_in_place_and_evicts_oldest_first() {
        let mut bakes = PortraitBakes::default();
        let mut images = Assets::<Image>::default();
        // Distinct real handles — a 1² target is the cheapest thing `Assets` will mint.
        let face = |images: &mut Assets<Image>| images.add(new_target_image(1));

        let faces: Vec<Handle<Image>> =
            (0..PortraitBakes::CAP).map(|_| face(&mut images)).collect();
        for (i, f) in faces.iter().enumerate() {
            bakes.store(i as u64 + 1, f.clone());
        }
        // A re-bake of the oldest entry: replaced, and it does NOT move up the eviction queue —
        // the reference re-stores the handle at `+0x24` and touches no ordering either.
        let rebaked = face(&mut images);
        bakes.store(1, rebaked.clone());
        assert_eq!(bakes.get(1), Some(rebaked));
        assert_eq!(
            bakes.order.len(),
            PortraitBakes::CAP,
            "a replace is not an insert"
        );

        // One new member past the cap evicts exactly the oldest.
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

    /// A camera-facing batch at attach `attach` — [`attach_id::SHOULDER_RIGHT`] by default, an id
    /// the reference's [`attach_reset`] keeps.
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

    /// A worn attach id the reset KEEPS — the right pauldron. Every fixture that isn't testing the
    /// reset itself hangs from it, so the reset never silently eats a fixture.
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

    /// What [`DressedLook::collect`] found, flattened to what the assertions care about.
    #[derive(Resource, Default)]
    struct Collected {
        parts: usize,
        riders: usize,
        billboards: Vec<PortraitSeat>,
        effects: Vec<(u16, usize)>,
        /// What the body panes would arm — [`hand_grip`] over the same collected set.
        grip: [bool; 2],
    }

    /// Run the walk over `unit` in `app` and hand back what it collected.
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

    /// **A mounted unit's booth keeps the rider's gear and drops the horse's.** The mount prune is
    /// what makes a portrait show the character alone (decision 0441) — but while mounted the
    /// character's own attach joints re-root INSIDE the mount subtree, so pruning everything under it
    /// would take the rider's equipment with the horse. Riders were already exempt; an equipped item's
    /// camera-facing batch and its effects have to be too (decision 0822), and the discriminator is
    /// [`PortraitSeat`] — whose model the batch belongs to — not where in the tree it happens to sit.
    #[test]
    fn a_mounts_own_glow_card_prunes_but_the_riders_gear_does_not() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let unit = app.world_mut().spawn(body_part()).id();
        // The character's own eye-glow, outside the mount subtree.
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
        // The mount's OWN glow card (its lantern) — a batch of the mount's rigged body.
        app.world_mut()
            .spawn((card(PortraitSeat::Body), ChildOf(mount)));
        // …and the seated rider's gear, re-rooted under the mount: a shoulder rider, its
        // camera-facing batch and its emitters.
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
        // Two `Body` cards exist in this tree — the character's eye-glow and the mount's lantern —
        // and exactly one may survive; the item's `Rider` card must, wherever it sits.
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

    /// A distinct mesh/material pair, so a fixture's identity is its own.
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

    /// A rider at `attach` with its own geometry.
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

    /// The three-slot dress of a hunter carrying a bow — unchanged by drawing it, because
    /// [`crate::entities::DressKey`] is read above the placement gate.
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

    /// **The half that removes the bow.** In combat the auto-draw is a *snap*
    /// (`SetSheatheState(…, bInstant != 0)`), which reaches `UNIT_MODEL_CHANGED`'s one fire site
    /// only through the enchant-gated `0x5eed50` — so the reference's doll never hears about it.
    /// Ours re-baked on the mirrored geometry, and a drawn bow IS mirrored geometry.
    ///
    /// The bow appears from nothing (a stowed ranged weapon renders nothing at all), so this is
    /// not a matter of ignoring a moved rider: the key has to be blind to the hand points, and to
    /// know the bow exists from somewhere else. That somewhere is the dress.
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

        // **The control, and the defect as a number.** The pane used to key on [`LookKey`] — the
        // mirrored geometry — and that is exactly what a draw moves. Without this the test above
        // asserts only that the new key ignores an input, never that the old one did not.
        let body = [body_part()];
        let body: Vec<&PortraitPart> = body.iter().collect();
        let bow: Vec<&PortraitRider> = bow.iter().collect();
        assert!(
            LookKey::build(&body, &[], &[], &[]) != LookKey::build(&body, &bow, &[], &[]),
            "the pre-1616 key moved on the draw — which is the bug"
        );
    }

    /// The melee twin: a stow does not delete the weapon, it re-parents it (0826) — same handles,
    /// a hand point for a hip point. Both ends are in the sheath lane, so neither is in the key.
    #[test]
    fn stowing_a_sword_does_not_re_snapshot_the_pane() {
        use crate::entities::attach_id::{HAND_RIGHT, HIP_MAIN};
        let sword = dress_of([Some((1234, ItemModelKind::Weapon, 0)), None, None]);
        let drawn = key(&[geom_rider(HAND_RIGHT, 7)], sword, 0, 1);
        let hipped = key(&[geom_rider(HIP_MAIN, 7)], sword, 0, 1);
        assert!(drawn == hipped);
    }

    /// …but the **manual** toggle does. `ToggleSheath` passes `bInstant = 0` on all three legs,
    /// which runs the ceremony, whose `$SHL`/`$SHR` keyframe marks the unit unconditionally
    /// (`0x5ffb10`, the mark at `0x5ffbbe`). benilla fires `SheathSwapMessage` at exactly that
    /// keyframe, and [`bump_model_revision`] turns it into a re-take. The director confirmed the
    /// observable: with the sheet open, Z moves the doll.
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

    /// Opening the window re-takes the model — the C++ show override `0x505d00`, not a Lua event
    /// (`PaperDollFrame_OnShow` calls no `SetUnit` at all). So a weapon drawn while the sheet was
    /// shut IS on the doll the next time you open it.
    #[test]
    fn showing_the_pane_re_snapshots_it() {
        use crate::entities::attach_id::HAND_LEFT;
        let open_stowed = key(&[], hunter(), 0, 1);
        let reopened_drawn = key(&[geom_rider(HAND_LEFT, 1)], hunter(), 0, 2);
        assert!(open_stowed != reopened_drawn);
    }

    /// Equipping is `UNIT_MODEL_CHANGED`'s unconditional producer (`0x5e2810` → `0x5dee30` →
    /// `0x5df119`), and it must reach the pane through the dress even when the new weapon is
    /// **stowed** — where it adds no mirrored geometry whatsoever.
    #[test]
    fn equipping_a_stowed_weapon_still_re_snapshots_the_pane() {
        let empty = key(&[], dress_of([None, None, None]), 0, 1);
        let armed = key(&[], hunter(), 0, 1);
        assert!(
            empty != armed,
            "the dress moved even though nothing is drawn"
        );
    }

    /// And the gear that cannot move with a sheath is keyed straight off the mirrored tree, so a
    /// helm arriving — or the armour composite re-blitting, which the reference shares by pointer
    /// and means to show on an open doll — re-takes without any event plumbing.
    #[test]
    fn a_helm_arriving_re_snapshots_the_pane() {
        use crate::entities::attach_id::HELM;
        let bare = key(&[], hunter(), 0, 1);
        let helmed = key(&[geom_rider(HELM, 9)], hunter(), 0, 1);
        assert!(bare != helmed);
    }

    /// **The nocked arrow never reaches the character-window doll.** A hunter shooting in
    /// combat has a bow at HandLeft(2) and an arrow at HandArrow(0x23); our booths mirrored both,
    /// and the reference's doll can never draw the arrow: every model widget duplicates the unit's
    /// attachment tree and then detaches `0x23` with fourteen other transient ids
    /// (`0x47a230`, [`attach_reset`]). The bow is NOT ours to hide here — `0x47a230` keeps the hand
    /// points — which is why this asserts both halves.
    #[test]
    fn the_nocked_arrow_never_reaches_a_booth_but_the_bow_in_hand_does() {
        use crate::entities::attach_id::{HAND_ARROW, HAND_LEFT};
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let unit = app.world_mut().spawn(body_part()).id();
        // The drawn bow: mesh parts at HandLeft.
        app.world_mut().spawn((rider_at(HAND_LEFT), ChildOf(unit)));
        // The nocked arrow, at the one body-bone attach the whole ammo mechanism uses.
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

    /// **The reset's membership, against the byte list.** `0x47a230` detaches fifteen ids —
    /// `0xf` twice, `0x10`, `0x11`, `0x12`, `0x13`–`0x19`, `0x1d`, `0x22`, `0x23` — and keeps
    /// everything else. Spelled out rather than re-derived, because getting the boundary wrong
    /// silently deletes worn gear from every pane (`0xb` helm, `0xc` cape, `5`/`6` shoulders) or
    /// silently keeps an effect.
    #[test]
    fn the_attach_reset_cuts_the_transient_family_and_keeps_equipment() {
        for kept in [
            0, 1, 2, // the three hand points — the whole reason the widget shows weapons
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

    /// **The grip is occupancy, not sheath state — and a shield never closes a hand.** The
    /// forearm point (`0`) is the shield's, and `0x479700`/`0x5059a0` fork on ids `1`/`2` alone
    /// (`0x60b678`: a shield in the offhand NEVER closes the hand, in any stance).
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

    /// **A wand is a camera-facing batch and an emitter set, with no mesh rider at all** — so a
    /// grip probe that only walked the mesh lane would leave the hand open around it. The
    /// reference probes the attachment NODE, which every lane of ours hangs from.
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

    /// **A key blind to effects would never re-bake for a glow.** An item glow resolves
    /// asynchronously (`entities::item_glow`: the item's template, then the effect model's own load),
    /// so its mirror lands *after* the meshes it rides with — a later frame, with every mesh handle
    /// identical. If the bake key ignored effects, the compare would say "unchanged" and the glow
    /// would stay out of the pane until some unrelated content edge forced a re-bake.
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

        // …and a *different* seat for the same emitter count is a different bake too: two glow slots
        // on one weapon differ by nothing else.
        let moved = effects(4, Vec3::new(0.21, 1.55, 0.06), 1);
        assert!(
            LookKey::build(&parts, &riders, &[], &[&moved]) != after,
            "the seat is part of the key",
        );
        // The same look twice is the same key — the compare must not re-bake every frame.
        assert!(LookKey::build(&parts, &riders, &[], &[&glow]) == after);
    }

    /// The 2D stand-in's player arm (`0x525ba0`): a unit the object manager
    /// does not hold draws `TemporaryPortrait-{Sex}-{Race}`. The out-of-area party member reaches
    /// it through the name cache's triple rather than a descriptor — same art, other source
    /// (report B315).
    #[test]
    fn the_stand_in_names_the_sex_and_race_and_falls_back_without_them() {
        assert_eq!(
            player_temporary_portrait(Some(4), Some(1)),
            "Interface\\CharacterFrame\\TemporaryPortrait-Female-NightElf.blp",
        );
        // Sex 0 is Male, and so is anything else the byte could hold.
        assert_eq!(
            player_temporary_portrait(Some(6), Some(0)),
            "Interface\\CharacterFrame\\TemporaryPortrait-Male-Tauren.blp",
        );
        // No name answer yet (or a race id the client has no string for): the plain file, which is
        // what the ref's `%s-%s` format degenerates to — NOT an empty circle, which is the bug.
        assert_eq!(
            player_temporary_portrait(None, None),
            "Interface\\CharacterFrame\\TemporaryPortrait.blp",
        );
        assert_eq!(
            player_temporary_portrait(Some(9), Some(1)),
            "Interface\\CharacterFrame\\TemporaryPortrait.blp",
        );
    }

    /// **A still is never committed while the render world is still building pipelines** —
    /// the symptom: the player's own portrait baking with the face (or the hair, or the shoulder)
    /// simply absent, at random, on Windows.
    ///
    /// The mechanism is not a missing asset and never shows up as one: off macOS Bevy builds each
    /// pipeline variant on the async pool, and `SetItemPipeline` answers a not-yet-built variant
    /// with `Skip` — the batch draws nothing, silently. Every live view redraws it a few frames
    /// later; a one-shot bake sleeps after [`BOOTH_SETTLE_FRAMES`] and keeps the hole for the
    /// session. Which batches lose the race is which variants were cold, which is why one bake
    /// loses the hair (the alpha-key pipeline) and the next the face (the opaque one).
    #[test]
    fn a_bake_holds_its_camera_awake_while_pipelines_are_still_building() {
        assert_eq!(pipe_settle(true, true, 0.0), PipeSettle::Hold);
        assert_eq!(pipe_settle(true, false, 0.0), PipeSettle::Hold);
    }

    /// **An idle cache does not spend the settle until the bake's own window has drained.** The
    /// counters cross worlds a frame behind, and the variants a bake needs are only queued once
    /// the render world has seen its meshes — so "the cache is idle" read on the frame of the
    /// bake is a fact about the frame *before* it. Spending there would restore exactly the bug.
    #[test]
    fn an_idle_reading_before_the_settle_window_drains_decides_nothing() {
        assert_eq!(pipe_settle(false, false, 0.0), PipeSettle::Idle);
        assert_eq!(pipe_settle(false, true, 0.0), PipeSettle::Spend);
    }

    /// The bound (the [`Booth::pipes_since`] doc): the cache is process-global, so a session that
    /// keeps meeting new variants can hold it non-empty indefinitely. The hold is released loudly
    /// rather than pinning a camera rendering for the rest of the session — the same shape, and
    /// the same reasoning, as [`PENDING_LANDING_SECS`].
    #[test]
    fn the_settle_is_bounded_even_while_the_cache_keeps_filling() {
        assert_eq!(
            pipe_settle(true, false, PIPELINE_SETTLING_SECS + 0.1),
            PipeSettle::Expired,
        );
        // …and the bound outranks the hold, or a busy cache would never reach it.
        assert_eq!(
            pipe_settle(true, true, PIPELINE_SETTLING_SECS + 0.1),
            PipeSettle::Expired,
        );
    }
}
