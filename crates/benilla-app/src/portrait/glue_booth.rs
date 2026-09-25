//! The GLUE booth — one booth slot serving both pre-world screens,
//! tuple-driven with **no wire entity**: the per-race `UI_<Race>` background scene with a character
//! standing in it — the character being *built* (char create, naked + underwear) or the selected
//! roster character (char select, geared from its enum record) — live, rotatable, fullscreen.
//!
//! The heavy lifting (displayId → model → geoset filter → composited skin → equipment riders) is
//! the entities module's [`crate::entities::attach`] glue-preview builder, which writes the
//! assembled part + rider lists into [`GluePreviewBake`] — it owns the attach path's resources, so
//! the booth doesn't re-implement them. This module owns the *booth* half, exactly like
//! [`super::sync_paperdoll`] does for the character window's paper doll: mirror the parts onto the
//! scene's authored light rig (or the studio buffer scene-less), pose a fresh instance via
//! [`super::spawn_booth_model`] — **looping Stand**, not a frozen still — seat riders on its
//! joints, and spin the root to the screen's live [`GluePreview::yaw`]. The bake lands in
//! [`super::PortraitImages`] under [`GLUE_SLOT`], sampled full-bleed by whichever glue screen is
//! up.

use benilla_assets::M2Model;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::PerspectiveProjection;
use bevy::prelude::*;
use bevy::render::render_resource::Extent3d;
use bevy::window::PrimaryWindow;

use crate::entities::Creatures;
use benilla_assets::m2_url;
use benilla_assets::materials::WowModelMaterial;

use super::framing::{
    attachment_point, diag_to_vert, glue_box_aspect, glue_box_physical, glue_scene_framing,
    ArtExtent, GLUE_AUTHORED_ASPECT, PORTRAIT_ASPECT,
};
use super::{
    aim, body_frame, new_target_image, spawn_booth_effects, spawn_booth_model, Booth,
    BoothBillboardSpec, BoothCam, BoothEffects, BoothInstance, BoothLight, BoothMotion, BoothPart,
    BoothRider, BoothTwins, Booths, PortraitImages, PortraitSource, VariantLane, GLUE_LAYER,
};

/// The glue booth slot token (its key in [`super::PortraitImages`] / [`Booths`]).
pub(crate) const GLUE_SLOT: &str = "glue";
// `GLUE_LAYER` is defined with the rest of the booth ladder in [`super`] — defining it here, from
// the same `PAPERDOLL_LAYER + 1` the inspect booth used, is exactly how the two collided.
/// The scene-less fallback target resolution — with a scene up the target resizes to the window
/// (the glue screens are fullscreen renders).
const GLUE_SIZE: u32 = 1024;

/// The character appearance the CREATE screen shows — race/sex/**class** + the five appearance
/// dials: the create body dressed in the (race, class, sex) level-1 starting
/// outfit (CharStartOutfit), the ref's create preview. Class is load-bearing here — a different
/// class wears different starting gear, and the ref re-applies equipment on class change
/// (`SelectClass 0x470f50` → `cc_apply_sections 0x470800`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct CreateLook {
    pub(crate) race: u8,
    pub(crate) sex: u8,
    pub(crate) class: u8,
    pub(crate) skin: u8,
    pub(crate) face: u8,
    pub(crate) hair_style: u8,
    pub(crate) hair_color: u8,
    pub(crate) facial_hair: u8,
}

/// The character the SELECT screen shows — the roster entry's full appearance + its visible
/// equipment display pairs, verbatim off `SMSG_CHAR_ENUM`.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct SelectLook {
    pub(crate) race: u8,
    pub(crate) sex: u8,
    pub(crate) skin: u8,
    pub(crate) face: u8,
    pub(crate) hair_style: u8,
    pub(crate) hair_color: u8,
    pub(crate) facial_hair: u8,
    /// `CHARACTER_FLAG_*` bits (hide-helm / hide-cloak honored by the builder).
    pub(crate) flags: u32,
    pub(crate) equipment: [benilla_protocol::CharEnumItem; 19],
    /// The roster row's **pet** triple — the hunter/warlock companion that stands beside the
    /// selected character. It rides the look, not a `GluePreview` field of its own, because it is
    /// part of the enum record and wants the same change detection every other select field gets:
    /// swapping rows re-bakes the pet with the body.
    ///
    /// All three words matter. The display names the model; the **level and family are its size**
    /// (`CreatureFamily`'s ramp), so a freshly tamed wolf stands visibly smaller
    /// than a level-60 one.
    pub(crate) pet_display_id: u32,
    pub(crate) pet_level: u32,
    pub(crate) pet_family: u32,
}

/// The select pet, as the booth needs it: which creature, and the two words that size it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PetLook {
    /// `CreatureDisplayInfo` id — never `0` here ([`GlueLook::pet`] is what filters that out).
    pub(crate) display_id: u32,
    pub(crate) level: u32,
    pub(crate) family: u32,
}

impl From<&benilla_protocol::Character> for SelectLook {
    fn from(c: &benilla_protocol::Character) -> Self {
        Self {
            race: c.race,
            sex: c.gender.min(1),
            skin: c.skin,
            face: c.face,
            hair_style: c.hair_style,
            hair_color: c.hair_color,
            facial_hair: c.facial_hair,
            flags: c.flags,
            equipment: c.equipment,
            pet_display_id: c.pet_display_id,
            pet_level: c.pet_level,
            pet_family: c.pet_family,
        }
    }
}

/// What stands on the glue stage: the create screen's dial tuple, or a select roster entry.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GlueLook {
    Create(CreateLook),
    Select(SelectLook),
}

impl GlueLook {
    /// The (race, sex) whose body model carries the look.
    pub(crate) fn body(&self) -> (u8, u8) {
        match self {
            GlueLook::Create(l) => (l.race, l.sex),
            GlueLook::Select(l) => (l.race, l.sex),
        }
    }

    /// The **pet** standing beside the character, or `None`. Only a select look can carry one: the
    /// create screen builds a character that has no pet yet, and the wire field it would come from
    /// doesn't exist until the character does.
    pub(crate) fn pet(&self) -> Option<PetLook> {
        match self {
            GlueLook::Create(_) => None,
            GlueLook::Select(l) => (l.pet_display_id != 0).then_some(PetLook {
                display_id: l.pet_display_id,
                level: l.pet_level,
                family: l.pet_family,
            }),
        }
    }
}

/// Which glue background scene shows: the login screen's main-menu gate, or a
/// race's `UI_<Race>` stage. An enum, not a fake race value — the login scene isn't a race, and
/// the fog law forks on the kind (MainMenu is always fogged with its authored XML fog; Race keeps
/// the 0429 create/select split).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlueScene {
    /// `UI_MainMenu` — the login screen's burning gate (`AccountLogin.xml`'s ModelFFX).
    MainMenu,
    /// A race's `UI_<Race>` stage (create/select; Gnome→Dwarf, Troll→Orc per the ref's mapping).
    Race(u8),
}

/// The glue screens' live input to the booth: which scene shows
/// (`None` = no scene, booth torn down — each screen drives it: login the main menu, create the
/// picked race, select the SELECTED character's race, Orc before a list arrives), the character
/// standing in it (`None` = empty stage — the login screen and an empty account), and the bake
/// **yaw** in radians (drag / rotate buttons write it; the create screen resets it to the ref's
/// −15° on entry, the select screen to 0). The look feeds the part builder; the yaw spins the
/// baked root (cheap — no re-bake).
#[derive(Resource)]
pub(crate) struct GluePreview {
    pub(crate) scene: Option<GlueScene>,
    pub(crate) look: Option<GlueLook>,
    pub(crate) yaw: f32,
}

impl Default for GluePreview {
    fn default() -> Self {
        Self {
            scene: None,
            look: None,
            yaw: 0.0,
        }
    }
}

/// The per-race glue **background scene** (the ref's fullscreen `ModelFFX` — `SetBackgroundModel`
/// loads `Interface\Glues\Models\UI_<Race>\UI_<Race>.mdx`, plays sequence 0, and shows camera 0):
/// the scene model spawns under its own root beside the character's (the character yaws; the scene
/// never does), the character stands on the scene's **attachment 0** (the stage spot, on camera 0's
/// axis in every UI_* scene), and while a scene is up its authored camera owns the booth camera.
/// Gnome shares Dwarf's scene and Troll shares Orc's (the ref's `SetBackgroundModel` mapping).
#[derive(Resource)]
pub(crate) struct CreateScene {
    root: Entity,
    token: Option<&'static str>,
    handle: Option<Handle<M2Model>>,
    spawned: bool,
    /// The authored camera 0, captured at spawn (drives the booth camera while the scene shows).
    cam: Option<benilla_assets::PortraitCamera>,
    /// How far the scene's art paints around that camera — the shipped scene's measured extent
    /// ([`benilla_formats::shipped_glue_art_extent`]) — the ceiling on the framing
    /// law's opening UPWARD on a narrow window ([`glue_scene_framing`]). `None` with no scene.
    /// The wide leg stopped reading it under 2187: one frame for every scene, so its width is
    /// [`super::framing::GLUE_BOX_ASPECT`]'s, not this stage's. It is still what says the frame
    /// is safe there (the spawn log below, and the test that pins the constant).
    art: Option<ArtExtent>,
    /// `Some(aspect)` while the frame pillarboxes the scene — the window is wider than
    /// [`super::framing::GLUE_BOX_ASPECT`]: the booth camera renders into a centred viewport of
    /// this aspect, black either side ([`pillarbox_glue_scene`]). `None`: the whole window.
    ///
    /// A property of the **window**, not of the scene — set from the window while
    /// any glue screen is up, cleared only when the booth is torn down. It is what the chrome's
    /// canvas insets by (2091), and 1619's per-scene box is why that canvas used to jump as the
    /// roster selection moved between races.
    viewport_aspect: Option<f32>,
    /// The character's stage spot — scene attachment 0 (Bevy model space), `ZERO` with no scene.
    /// (At select too: the body seats on attachment **0**, `0x473039` — attachment 1 is the
    /// enum's secondary model, `0x47306b`; 0429's attachment-1 presumption is dead.)
    char_spot: Vec3,
    /// The stage's **authored frame** — attachment 0's bone matrix ([`stage_frame`]): the rotation
    /// and uniform scale the scene author parked on that bone. Identity/1.0 with no scene;
    /// UI_Human's holds the −16.5° yaw that aims the character at that scene's off-axis camera
    /// (and a 0.97 scale), UI_Tauren's a 1.04 scale (1533).
    char_facing: Quat,
    char_scale: f32,
    /// The **pet's** spot — scene attachment 1 (`0x47306b` chains the secondary model there), a few
    /// yards off the stage. Every `UI_*` scene authors exactly these two attachments and no more,
    /// and both their bones are parentless roots — so the seat is that bone's own frame, with
    /// nothing above it to compose, which is why the pet is placed the same way the body is.
    pet_root: Entity,
    pet_spot: Vec3,
    /// The pet's share of the stage's frame — attachment **1**'s bone, read the same way
    /// [`Self::char_facing`] is. 1536 placed the pet with a pure translation on the reading that
    /// both attachment bones are unkeyed; that holds on five scenes and **not** on two of them.
    /// UI_Human's attachment-1 bone (bone 5) carries the identical constant −16.5° yaw and 0.97
    /// scale its attachment-0 bone does — without the yaw the pet stands square beside an owner
    /// turned 16.5° away — and UI_Tauren's parks a **0.7324** scale on the pet's bone alone, three
    /// quarters the size we were drawing it (1533).
    pet_facing: Quat,
    pet_stage_scale: f32,
    /// The pet bake standing at [`Self::pet_spot`] — its revision, so a re-bake happens once.
    pet_baked: Option<u64>,
    /// Whether the current spawn wrote the fog rows enabled (the create law) — a screen hop with
    /// the same scene race still rebuilds when this flips.
    fog: bool,
    /// Whether the light buffer currently holds the **ghost** rig ([`ghost_rig`]) rather than the
    /// scene's authored one. Unlike [`Self::fog`] a change here does NOT rebuild the scene: the
    /// reference's fork is a per-frame fill callback on models that are already standing, so the
    /// buffer is rewritten in place and the stage's braziers keep burning across the click.
    ghost: bool,
    /// The scene's own light buffer — the scene's **authored M2 light rig** folded per the world
    /// law (see [`SceneRig`]: directionals → ambient + SH probe slot 0, points → the per-vertex
    /// point table) plus the ref's per-race fog (`CharModelFogInfo`) — **create only**: at select
    /// the client overwrites the background's fog callback with the light callback, so the select
    /// scene renders unfogged (`0x472110`) —
    /// rewritten on each scene swap. The character re-lights onto it too — the ref's create character is scene-lit (the
    /// NightElf/Scourge characters are lit almost entirely by stage-local point lights).
    light: Option<bevy::render::render_resource::Buffer>,
    /// Booth twins of the character's materials against the scene buffer (the same dedup law as
    /// `BoothLight::variants`; one rewritten buffer per race keeps entries valid across swaps).
    variants: std::collections::HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
    /// Bumped on every scene spawn — folded into `sync_glue_booth`'s change key so the character
    /// re-lights the moment the scene (and its rig) lands.
    rev: u64,
    /// **The stage the selection has asked for but has not been given yet** — see
    /// [`PendingSwap`]. `None` = the standing scene IS the requested one.
    pending: Option<PendingSwap>,
}

/// A requested stage swap, waiting for its half of the pair.
///
/// **The two halves of a glue booth swap independently, and that is the whole defect.** Clicking a
/// row rewrites [`GluePreview`] in one frame; the background scene is an M2 that loads in one or
/// two more, and the character is an *assembly* — body display, composited skin, every worn item's
/// model — that [`crate::entities::attach::preview::build_glue_preview`] deliberately withholds
/// until all of it is ready. Whichever half wins is what the screen shows in the meantime:
///
/// ```text
/// t=8.484  click Orcshafour (orc)          ← from a human
/// t=8.487  create scene: UI_Orc up
/// t=8.487  [booth] glue rebake display=49  ← the HUMAN, standing in the ORC stage, by=scene
/// t=8.540  [booth] glue rebake display=51  ← 53 ms later, the orc
/// ```
///
/// and the reverse order for a body whose model is already cached under a stage whose art is not
/// (measured the same run, 34 ms of a human standing in the orc stage). That is the director's
/// *"switching between chars that haven't been loaded before briefly shows a different model —
/// a clear jump from something else to what it eventually turns out to look like"*.
///
/// The reference cannot express this state: `SelectCharacter` loads the background model and
/// builds the character inside one blocking call, so the pair is atomic by construction. Ours
/// makes it atomic by **holding the committed pair until the requested one is whole** — the loads
/// still start the instant the row is clicked, so nothing is slower; only the *swap* waits.
struct PendingSwap {
    /// The requested scene token (`UI_<token>`).
    token: &'static str,
    /// Its model, loading since the click — the hold is about when the stage swaps, never about
    /// when its art starts arriving.
    handle: Handle<M2Model>,
    /// `Time<Real>` at the click, for [`GLUE_SWAP_HOLD_SECS`].
    since: f32,
}

/// 0737's never-hold-unbounded rule, applied to the swap above: a look whose assembly can never
/// complete (a display the cache will not resolve, an item model that is not there) must not
/// freeze the screen on the previous character. Generous, because the cost of waiting is a stale
/// but COHERENT stage and the cost of releasing early is exactly the defect.
const GLUE_SWAP_HOLD_SECS: f32 = 2.0;

/// The scene's model-name token: `Interface\Glues\Models\UI_<token>\UI_<token>.mdx`. The race
/// mapping is the ref's (`GlueParent.lua`'s `SetBackgroundModel` HACK block); the main menu is
/// `AccountLogin.xml`'s own model.
fn scene_token(scene: GlueScene) -> &'static str {
    match scene {
        GlueScene::MainMenu => "MainMenu",
        GlueScene::Race(race) => match race {
            2 | 8 => "Orc",   // Troll shares Orc's scene
            3 | 7 => "Dwarf", // Gnome shares Dwarf's
            4 => "NightElf",
            5 => "Scourge",
            6 => "Tauren",
            _ => "Human",
        },
    }
}

/// **May the stage swap to the requested scene this frame?** — [`PendingSwap`]'s whole law, as one
/// function, because *when a glue selection is allowed to become visible* is the thing that was
/// wrong and it should be readable (and testable) without standing up a booth.
///
/// - `standing` — is there a coherent pair on the stage to protect? With nothing standing there is
///   no mispairing to avoid and the swap is free (the login → select hop, an empty account).
/// - `art_ready` / `body_ready` — the requested scene's model is loaded, and the requested look's
///   assembly has landed.
/// - `waited` — seconds held, against [`GLUE_SWAP_HOLD_SECS`]'s fail-open bound (0737's rule: a
///   cover — or a hold — is never unbounded).
fn may_swap(standing: bool, art_ready: bool, body_ready: bool, waited: f32) -> bool {
    !standing || (art_ready && body_ready) || waited >= GLUE_SWAP_HOLD_SECS
}

/// A glue scene's authored M2 light rig, folded for the booth's light buffer under the
/// world law (the gather `0x718960`): **directional** lights accumulate —
/// ambient tracks summed into one ambient term, each diffuse×intensity an SH lobe with its *own*
/// direction (the `Model2.bls` fold, [`benilla_world::lighting::prop_probe_coeffs`]) — and **point**
/// lights become per-vertex lights on the buffer's point table, evaluated with the engine's fixed
/// falloff `1/(0.7·d + 0.03·d²)` (the authored attenuation radii are dead code in the reference;
/// `wow_model.wgsl`'s point term already implements the law). Both booth models read this one
/// rig — the scene at its origin, the character at the stage — exactly the reference's shared
/// scene-light gather. Light positions are authored in model space (the attachment convention),
/// bind-pose static in every UI_* scene.
struct SceneRig {
    /// The summed directional-light ambient (every UI_* point light authors ambient ×0).
    ambient: [f32; 3],
    /// The directional lobes, toward-light and committed — folded into probe slot 0.
    lobes: Vec<(Vec3, [f32; 3])>,
    /// The point lights: Bevy position + committed colour.
    points: Vec<(Vec3, [f32; 3])>,
}

/// Candidacy range packed with a scene point light — the reference gather has no range gate
/// (pure ≤3-nearest), so effectively unbounded; the falloff does the bounding.
const SCENE_POINT_RANGE: f32 = 1.0e6;

/// Fold the scene model's light table into a [`SceneRig`] (see there for the law). A directional
/// light's **to-light direction is its bone's local +Z axis** (`M2Light::bone_z` — the gather
/// reads row 2 of the light bone's pose matrix at `0x718a76`, never the def position; on-surface
/// to-light = `normalize(Bz·BM_rot)`); the model rotation `BM_rot` is identity here (the scene
/// root never rotates).
fn scene_rig(lights: &[benilla_assets::ModelLight]) -> SceneRig {
    let mut ambient = [0.0f32; 3];
    let mut lobes: Vec<(Vec3, [f32; 3])> = Vec::new();
    let mut points = Vec::new();
    for l in lights.iter().map(|l| &l.def) {
        for (a, c) in ambient.iter_mut().zip(l.ambient_color) {
            *a += c * l.ambient_intensity;
        }
        let color = l.diffuse_color.map(|c| c * l.diffuse_intensity);
        if l.is_point() {
            // `LightBlob::point` applies the same RAW commit the world table does (the `0x71ca80`
            // encode is undone by the `0x593040` decode): a glue scene is just another CM2Scene,
            // and the GL commit is shared. One law covers both tables, not two by coincidence.
            points.push((benilla_assets::coords::wow_to_bevy(l.position), color));
        } else {
            // Toward-light unit vector, as the probe fold expects (it re-normalizes).
            lobes.push((benilla_assets::coords::wow_to_bevy(l.bone_z), color));
        }
    }
    SceneRig {
        ambient,
        lobes,
        points,
    }
}

/// **The GHOST select screen's hardcoded sun** — the one directional the reference injects when the
/// selected roster record carries `CHARSELECT+0xfc & 0x2000`, replacing the authored rig entirely.
///
/// `0x472150` is a per-model per-frame **fill callback** (`[model+0x3bc]`, registered by `0x7134b0`)
/// whose *entire body* is gated on that bit (`47218e mov ecx,[record+0xfc]; 472194 test ch,0x20;
/// 472197 je 0x4722b2`). It is registered on **three** models — the background scene M2
/// (`0x472144`), the character (`0x472bac`) and the **pet** (`0x472ebd`) — so all three fork
/// together off one `[0x83856c]`-keyed read. For a living selection the callback is a complete
/// no-op and the authored rig stands, exactly as at create (decision 0435's law, select-confirmed).
///
/// For a ghost it does two things, and this function is both:
///
/// - **the collector is WIPED** — `0x71bc30` opens `mov ecx,0x6d; rep stosd`, 436 bytes zeroed, so
///   the scene-DB gather is discarded: no authored ambient, and **no point lights** (which is why
///   the ghost stage loses the NightElf/Scourge stage-local lamps that make those two scenes);
/// - **one hardcoded directional is injected** (built by `0x71b4a0`, forced directional by
///   `0x71b620(0)`, colours by two `0x6d6ad0()→0x6d62e0()` DN reads, specular left 0).
///
/// **The direction, and the trap in it.** `0x472280 mov [ebp-4],0xbf800000` writes the raw literal
/// `(0,0,-1)` into `CGLight+0x24`, which is a **from-light** vector: `0x71b6a0` stores it verbatim
/// (normalize only, no `fchs`) and `0x71bce0` negates all three components (`0x71be7c`/`0x71be81`/
/// `0x71be86`) before accumulating the SH lobe, so the lobe is centred on `-u` = **`(0,0,+1)`, i.e.
/// lit from directly overhead** (WoW Z up). The fixed-function lane agrees — `0x59c820` negates
/// too, and GL's directional `GL_POSITION` *is* the to-light vector — so the sign does not depend
/// on which lane you emulate. The trap: an **authored** M2 light has TWO negations (`0x718960`
/// already stores `-Bz`, and [`scene_rig`] feeds `+bone_z` as its to-light to match), while this one
/// has exactly ONE. Carrying the authored convention across double-negates and lights the ghost
/// from below.
///
/// **The colours are DBC data, not a live capture.** `0x4721e2 mov esi,[edx+0xc]` hard-codes
/// **`LightParams` row 3** — `Light.dbc`'s **death** preset column (id 3 in 59 of 374 rows and in no
/// other column; the world's ghost arm selects the same slot, `0x5de9df mov ecx,4; call 0x6d4620`)
/// — and `0x6d6ad0` maps it to `LightIntBand` ids `18·3−17+sub`: **37 diffuse** (colour sub 0),
/// **38 ambient** (sub 1), sampled at a hardcoded time key of `0` (`xor edx,edx`; half-minutes,
/// `[0, 2880)`). Both rows are **single-key in the shipped 1.12.1 chain**, hence time-invariant,
/// hence these literals — 0469 parked this as "pending a live capture" on the earlier reading that
/// the tables were runtime-only, and that park is withdrawn.
///
/// The values are stated rather than read back out of our own `Light.dbc` reader for one reason:
/// the glue screens run **pre-world**, where no map, no zone and no `LightParams` state exist at
/// all — the reference reaches them through the DN singleton it keeps alive across the glue, and we
/// have no such thing to ask. A chain whose bands are keyed (a patched install) would diverge here;
/// that is the stated bound.
fn ghost_rig() -> SceneRig {
    SceneRig {
        // `LightIntBand` 38 = 0x001A3855 → (26, 56, 85).
        ambient: [26.0 / 255.0, 56.0 / 255.0, 85.0 / 255.0],
        // `LightIntBand` 37 = 0x005E99C6 → (94, 153, 198), on the to-light `(0,0,+1)`.
        lobes: vec![(
            benilla_assets::coords::wow_to_bevy([0.0, 0.0, 1.0]),
            [94.0 / 255.0, 153.0 / 255.0, 198.0 / 255.0],
        )],
        // Wiped: the gather is discarded whole, so the stage's own lamps do not survive.
        points: Vec::new(),
    }
}

/// Pack a [`SceneRig`] into the glue scene's light buffer contents.
///
/// Rig materials read the probe + point table; the core rows still carry the plain ambient (with a
/// zero sun) so any non-rig lane that ever lands in the booth degrades sanely. The directional fold
/// lands in probe slot 0 — booth instances carry `MeshTag` 0, the rig lane's read side. The dial is
/// 1.0: the rig lane ignores it, so byte levels stand.
///
/// One function for both rigs, so the authored and the ghost light cannot drift into two packings.
fn scene_light_blob(
    rig: &SceneRig,
    fog_rgb: [f32; 3],
    fog_far: f32,
    fog: bool,
) -> benilla_world::lighting::LightBlob {
    let mut blob = benilla_world::lighting::LightBlob::model(rig.ambient, [0.0; 3], Vec3::NEG_Y)
        .probe(rig.ambient, &rig.lobes)
        .fog(fog_rgb, fog_far, fog)
        .dial(1.0);
    for (pos, color) in &rig.points {
        blob = blob.point(*pos, SCENE_POINT_RANGE, *color);
    }
    blob
}

/// The ref's per-scene fog: the race rows are `CharModelFogInfo` (GlueParent.lua) — color + far,
/// near always 0; the main menu's is `AccountLogin.xml`'s own `<FogColor>` + `fogFar` attribute.
fn scene_fog(token: &str) -> ([f32; 3], f32) {
    match token {
        "MainMenu" => ([0.25, 0.06, 0.015], 1200.0),
        "Orc" => ([0.5, 0.5, 0.5], 270.0),
        "Dwarf" => ([0.85, 0.88, 1.0], 500.0),
        "NightElf" => ([0.25, 0.22, 0.55], 611.0),
        "Scourge" => ([0.0, 0.22, 0.22], 26.0),
        "Tauren" => ([1.0, 0.61, 0.42], 153.0),
        _ => ([0.8, 0.65, 0.73], 222.0), // Human
    }
}

/// One assembled preview mesh — the same shape as a mirrored [`super::PortraitPart`], but produced
/// tuple-driven (no entity) by the entities-side builder: both mesh twins + the steady, world-lit
/// material (the booth re-lights it studio via [`BoothLight`]).
#[derive(Clone)]
pub(crate) struct PreviewPart {
    pub(crate) static_mesh: Handle<Mesh>,
    pub(crate) skinned_mesh: Option<Handle<Mesh>>,
    pub(crate) material: Handle<WowModelMaterial>,
    /// The batch's **animated material alpha**, carried to the booth's [`BoothPart`]. The CHARACTER
    /// assembly still supplies `None` here — threading it through the appearance layer is decision
    /// 0807's named gap, and the select body's look is signed off as it stands. The **pet** builder
    /// has the field in hand (it reads the display's `EntityPart`s straight, with no compositor in
    /// between), so it fills it.
    pub(crate) alpha_anim: Option<std::sync::Arc<benilla_formats::AlphaAnim>>,
    /// The batch's translucency twins ([`BoothTwins`]) — carried so a bake whose INSTANCE alpha is
    /// below 1 (the ghost select, [`GhostKit`]) can draw through the blend twin and arm the depth
    /// prime. Empty for a look that cannot feather; the booth relights whatever is here beside the
    /// steady material.
    pub(crate) twins: BoothTwins,
}

/// One equipment rider on the assembled preview (helm / shoulder / sheathed weapon — decision
/// 0465): the item model part's mesh + built material, seated under the body skeleton's `bone`
/// joint at the attach point's `offset` — the booth twin of the world path's
/// [`crate::portrait::PortraitRider`].
#[derive(Clone)]
pub(crate) struct PreviewRider {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) material: Handle<WowModelMaterial>,
    pub(crate) bone: u16,
    pub(crate) offset: Vec3,
    /// See [`PreviewPart::twins`] — an attached model composes onto its wearer's instance alpha.
    pub(crate) twins: BoothTwins,
}

/// One **camera-facing batch** on the assembled preview — the world path's billboard card
/// ([`benilla_world::billboard`]). The booth is a separate camera, so such a batch can't ride the world card
/// system; it's rebuilt here for the booth's own camera ([`super::booth::face_booth_billboards`]).
/// Its centred quad, its material, and the billboard bone/flag it rides.
///
/// Three sources, all seated the same way — under the body bone's joint at [`Self::offset`]:
///
/// - The character's own **eye-glow** (undead/night-elf, geoset 302). Seated on its billboard
///   bone's joint, whose frame already bakes the bone pivot (the 0130 rig identity), so its offset
///   is `ZERO` — the rigged case, exactly like the world's `BillboardCard::following_joint`.
/// - An **equipped item's** camera-facing batch — a wand's gem, a glowing rune on a weapon. 270 of
///   the 2681 `Item\` models author one. An item model gets no rig here, so its offset carries the
///   attach point *and* the batch's own model-local pivot.
/// - An **item glow's** camera-facing batch — `Spells\Enchantments\Sparkle_A.m2` is
///   exactly one such quad and nothing else, and `ItemVisuals` 28 (10 item displays) hangs it. Same
///   rig-less seat, plus the glow slot's offset on the weapon model.
#[derive(Clone)]
pub(crate) struct PreviewBillboard {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) material: Handle<WowModelMaterial>,
    pub(crate) bone: u16,
    /// Where the card's **pivot** sits in that bone's joint frame (Bevy axes). `ZERO` for a batch of
    /// the rigged body itself — its joint already bakes the pivot.
    pub(crate) offset: Vec3,
    pub(crate) kind: benilla_formats::BillboardKind,
    /// See [`PreviewPart::twins`] — a card is a batch of the model like any other.
    pub(crate) twins: BoothTwins,
}

/// One **effect-bearing model** on the assembled preview, carried as its particle emitters plus the
/// composed seat: the BODY bone it rides and its offset expressed in that bone's frame. The booth
/// spawns one host there and owns the emitters off it, exactly as [`sync_glue_scene`] does for a
/// backdrop's braziers; any geometry the same model carries rides the ordinary [`PreviewRider`]
/// list at the same seat.
///
/// Two sources produce these, and the booth treats them identically:
///
/// - **An equipped item's own emitters** — the R14 PVP pauldron's `SPARKLE` twinkle, the held
///   torch's flame (the select screen showed
///   nothing at all until it was carried here). Seat = the body's attach point for that slot.
/// - **An item glow** — the `Spells\Enchantments\*.mdx` effect a held weapon's
///   `ItemVisuals` id hangs on the weapon's own attachment point; all but three of the 35 shipped
///   glow models are pure emitters. Seat = the body attach point **plus** the slot's offset on the
///   weapon model.
#[derive(Clone)]
pub(crate) struct PreviewEffects {
    pub(crate) bone: u16,
    pub(crate) offset: Vec3,
    pub(crate) emitters: Vec<benilla_assets::ModelEmitter>,
}

/// The select screen's **ghost** treatment on the character body — what the reference's
/// `0x4727f0` applies when the selected roster record carries `CHARSELECT+0xfc & 0x2000`
/// (the row text is the same bit, `refresh.rs`).
///
/// **It is one hard-coded `SpellVisualKit` row, and we read it from the DBC exactly as the
/// reference does.** `0x47280f` loads `idmap[989]` behind the guard `0x4727f6 cmp ds:0xc0d750,0x3dd`
/// — the *id* is the constant, the values are data — and 989 is the state kit of `SpellVisual` 886,
/// which is the visual of spell 8326 "Ghost", the very aura the WORLD ghost rides
/// (`crate::aura_visual`, `0x5ff350`). So the select mannequin and a
/// released ghost in the world are the same kit reaching the same three render properties by two
/// different code paths, and benilla resolves both through the one dispatch point
/// ([`crate::aura_visual::node_for`]) rather than transcribing constants twice.
///
/// The shipped 5875 values, for orientation only — nothing here hardcodes them:
/// `charProc = {1, 14, -1, -1}`, `charParamZero = {9222653.0, 0.5, …}` → tint `0x8CB9FD`
/// = RGB (140, 185, 253), alpha 0.5, one attached effect — `Spells\Ghost_state.mdx` at
/// attachment `0x13` (Base). That model is emitter-only (0 vertices, 5 bones, one
/// `PARTICLES\FLAME02.BLP`), so it reaches the booth as [`PreviewEffects`] and nothing else —
/// but the resolution below carries geometry and cards too, because what the row names is data.
///
/// **No ramp.** The world's proc-14 alpha eases in over 1000 ms (`StartAlphaFade 0x614f80`); the
/// select path never reaches that block — `0x4727f0` writes `cc+0x180`/`+0x184..0x18c` straight
/// through `0x710cb0`/`0x710cf0`. A selection is instantly ghosted, and re-selecting is instant too.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct GhostKit {
    /// The whole-model modulate colour (CharProc 1 — `cc+0x184..0x18c`), as the packed byte triple
    /// [`benilla_world::instance_tint::pack`] wants.
    pub(crate) tint: [u8; 3],
    /// The whole-model alpha factor (CharProc 14 — `cc+0x180`), applied instantly.
    pub(crate) alpha: f32,
}

/// The entities-side builder's output: the geoset-filtered, composited
/// parts for the current look (+ its equipment riders for a Select look), plus the body displayId
/// (for the booth's framing/rig) and a revision that bumps on every real change — a new successful
/// bake, or a clear to `look: None`. The booth re-bakes only when `revision` changes; a bare yaw
/// change never touches it.
#[derive(Resource, Default)]
pub(crate) struct GluePreviewBake {
    pub(crate) look: Option<GlueLook>,
    pub(crate) display_id: u32,
    pub(crate) parts: Vec<PreviewPart>,
    pub(crate) riders: Vec<PreviewRider>,
    /// Every emitter-bearing model the look wears — the equipped items' own effects
    /// and the held items' glows. See [`PreviewEffects`].
    pub(crate) effects: Vec<PreviewEffects>,
    /// The look's camera-facing batches (the eye-glow, a wand's gem, a glow model's quad) — seated at
    /// their seat under a body joint and faced to the booth camera, not drawn as plain meshes. See
    /// [`PreviewBillboard`].
    pub(crate) billboards: Vec<PreviewBillboard>,
    /// Per-hand weapon grip `[right, left]` — a hand whose attach point holds a weapon closes into the
    /// `HandsClosed` finger pose (`CloseHand 0x479660`). The builder knows the held items'
    /// attach ids (which the flat rider list drops), so it resolves the grip here for the booth spawn.
    pub(crate) grip: [bool; 2],
    /// The selected character's **ghost** kit, or `None` for a living selection — see [`GhostKit`].
    /// Its attached effect models are already folded into the lists above (they are ordinary
    /// riders/cards/emitters at a body attach point); what stays here is what the *body itself*
    /// renders with.
    pub(crate) ghost: Option<GhostKit>,
    pub(crate) revision: u64,
}

/// The select screen's **pet** — the hunter/warlock companion standing beside the selected
/// character (`SMSG_CHAR_ENUM`'s pet triple, the reference's "secondary model" `record+0x114`).
///
/// Its own resource, and its own revision, deliberately: the pet's model can land frames after the
/// body's, and folding it into [`GluePreviewBake`] would make that late arrival re-bake the
/// character too — restarting an already-correct Stand loop for nothing. Empty (`display_id == 0`)
/// whenever the look carries no pet, which is every character but a living hunter's or warlock's
/// (the server suppresses the triple for the rest — see [`benilla_protocol::Character`]).
#[derive(Resource, Default)]
pub(crate) struct GluePetBake {
    /// The pet's `CreatureDisplayInfo` id — `0` = no pet, and the booth stands empty.
    pub(crate) display_id: u32,
    pub(crate) parts: Vec<PreviewPart>,
    /// The pet's camera-facing batches — the voidwalker's eyes are exactly this.
    pub(crate) billboards: Vec<PreviewBillboard>,
    /// The pet model's **own** particle emitters, straight off the display cache — the imp's three
    /// green flame jets (bones 17/45/46, `FlameLickSmall`, rate keyed alive in every sequence), the
    /// felhunter's, the succubus's. Unlike the body's [`GluePreviewBake::effects`] — which are an
    /// *equipped item's* emitters, seated on a character bone through a host — these are authored on
    /// the pet's own skeleton, so they ride its own joints: the scene-backdrop recipe, not the
    /// worn-item one ([`sync_glue_pet`]).
    pub(crate) emitters: Vec<benilla_assets::ModelEmitter>,
    /// The uniform render scale — the **`CreatureFamily` level ramp**, `minScale` → `maxScale`
    /// across `minScaleLevel` → `maxScaleLevel`
    /// ([`benilla_formats::CreatureFamily::scale_at`]).
    ///
    /// Not the `CreatureModelData.modelScale × CreatureDisplayInfo.scale` product this field
    /// briefly held: the reference computes that product one instruction before the ramp and then
    /// overwrites it (`0x472e87 fstp`), so the product is only ever the fallback for a family the
    /// table doesn't know. Either way the glue pet has no wire object, so — unlike every unit in
    /// the world — nothing has folded a scale in for it and the client must size it itself.
    pub(crate) scale: f32,
    pub(crate) revision: u64,
}

/// Stand up the create booth beside the portrait/paper-doll ones (called from [`super::setup_booths`],
/// which owns the shared image/booth resources). Same off-screen pipeline as the paper doll — HDR +
/// FfxGlow, negative order — but a [`GLUE_SIZE`] target cleared *transparent*, its own layer,
/// framed per-bake.
pub(super) fn spawn_glue_booth(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    portraits: &mut PortraitImages,
    booths: &mut Booths,
) {
    let image = images.add(new_target_image(GLUE_SIZE));
    portraits
        .0
        .insert(GLUE_SLOT.to_string(), PortraitSource::Live(image.clone()));
    let layer = RenderLayers::layer(GLUE_LAYER);
    let root = commands
        .spawn((Transform::IDENTITY, Visibility::Visible, layer.clone()))
        .id();
    commands.spawn((
        // The shared booth view shape (HDR, no tonemap, no MSAA) — its doc in [`super`] says why
        // every booth camera must spawn through it.
        super::booth_view_shape(),
        Camera {
            order: -100 + GLUE_LAYER as isize,
            // Transparent (unlike the portrait/paper-doll booths' backdrop): the create screen
            // composites the character straight over the page, the ref's model-in-the-scene look.
            // FfxGlow's combine carries the scene alpha through for exactly this.
            clear_color: ClearColorConfig::Custom(Color::NONE),
            // The OUTPUT clear — what the upscaling blit paints on the target OUTSIDE the
            // camera's viewport, when the framing law boxes the scene (
            // [`pillarbox_glue_scene`]). Bevy's default here is the global `ClearColor`, which
            // the world's lighting sets to the day's fog colour every frame — so without this
            // the bars came out Elwynn-sky blue. Black: a frame, not a page showing through.
            // With no viewport the blit covers the whole target and this is never seen.
            output_mode: bevy::camera::CameraOutputMode::Write {
                blend_state: None,
                clear_color: ClearColorConfig::Custom(Color::BLACK),
            },
            ..default()
        },
        bevy::camera::RenderTarget::Image(image.clone().into()),
        // **The glue screens run the reference's own glue pass pair**, not a bake's nothing:
        // this camera draws the fullscreen `UI_<Race>` diorama the reference
        // brackets its glue FFX around (`0x46fad3`/`0x46fae0`), and the GlueXML frames composite
        // over the result afterwards — which is why a ghost selection washes the scene steel-blue
        // while the character list, the buttons and the logo stay clean.
        benilla_world::ffx_glow::FfxGlow::GLUE_SCENE,
        // Placeholder — `sync_glue_booth` overwrites transform + projection from the model's bounds
        // on the first bake (the same `body_frame` law as the paper doll).
        Projection::from(bevy::camera::PerspectiveProjection {
            fov: super::PORTRAIT_FOV,
            near: 0.02,
            far: 100.0,
            ..default()
        }),
        layer.clone(),
        BoothCam(GLUE_SLOT.to_string()),
    ));
    booths.0.insert(
        GLUE_SLOT.to_string(),
        Booth {
            layer: layer.clone(),
            root,
            target: image,
            baked: None,
            baked_guid: None,
            snap: None,
            shown: false,
            show_rev: 0,
            // The glue camera is scene-driven, not wake-driven: `gate_booth_cameras` keeps it
            // active exactly while a glue scene shows (the scene is LIVE — looping animation,
            // global sequences, particle emitters — so it renders continuously, unlike the stills).
            wake: 0,
            // …which is `live_scene` in the gate, keyed on a scene being up rather than on this
            // flag: the glue booth is live from the moment a screen shows, before any bake.
            live: false,
            pending: Vec::new(),
            pending_since: None,
            pipes_settling: false,
            pipes_since: None,
            aspect: 1.0,
            rigged: false,
            parked: false,
            turn: super::Turn::default(),
        },
    );
    // The background scene's own root (the character root above yaws; the scene never does).
    let scene_root = commands
        .spawn((Transform::IDENTITY, Visibility::Visible, layer.clone()))
        .id();
    // …and the pet's, a third sibling: it stands on its own scene attachment and, like the scene,
    // never yaws — the reference's facing call touches only the character component's model
    // (`0x4730e0` → `cc+0x38`), so dragging the character spins the character alone.
    let pet_root = commands
        .spawn((Transform::IDENTITY, Visibility::Visible, layer))
        .id();
    commands.insert_resource(CreateScene {
        root: scene_root,
        token: None,
        handle: None,
        spawned: false,
        cam: None,
        art: None,
        viewport_aspect: None,
        char_spot: Vec3::ZERO,
        char_facing: Quat::IDENTITY,
        char_scale: 1.0,
        pet_root,
        pet_spot: Vec3::ZERO,
        pet_facing: Quat::IDENTITY,
        pet_stage_scale: 1.0,
        pet_baked: None,
        fog: true,
        ghost: false,
        light: None,
        variants: default(),
        rev: 0,
        pending: None,
    });
}

/// Drive the glue screens' **FFX state** ([`benilla_world::ffx_glow::GlueFfx`]) off the live look —
/// which pass the glue pair has installed, and what `LightParams.glow` is pinned to.
///
/// One writer for all three glue screens, because the reference's three writers are all reached
/// through the same widget and the same build:
///
/// | look | reference | pass | glow |
/// |---|---|---|---|
/// | none (login, empty account) | `CSimpleModelFFX::OnShow 0x46fa60` | glow | `*0x8380b4` 0.30 |
/// | a create look | same — no select build runs | glow | 0.30 |
/// | a living select record | `0x472ff7`/`0x473002` | glow | `*0x838574` 0.40 |
/// | a ghost select record | `0x472fde`/`0x472fe9` | **death** | `*0x838570` 0.15 |
///
/// An empty account really is the OnShow state and not the living arm: `0x472950` is gated
/// `-1 ≤ [0x83856c] < [0xb42140]`, so with no roster nothing re-pins anything.
///
/// It reads [`GluePreview`] rather than the roster because that is what every glue screen already
/// writes — the login screen's `None`, the create screen's `Create` look, the select screen's
/// `Select` record — so no screen can forget to reset it on the way out.
pub(super) fn sync_glue_ffx(
    preview: Res<GluePreview>,
    mut ffx: ResMut<benilla_world::ffx_glow::GlueFfx>,
) {
    use benilla_world::ffx_glow::GlueFfx;
    let want = match preview.look {
        Some(GlueLook::Select(l)) => {
            if l.flags & benilla_protocol::CHARACTER_FLAG_GHOST != 0 {
                GlueFfx::SelectGhost
            } else {
                GlueFfx::SelectLiving
            }
        }
        Some(GlueLook::Create(_)) | None => GlueFfx::Shown,
    };
    if *ffx != want {
        *ffx = want;
    }
}

/// Drive the background scene (see [`CreateScene`]): load the selected race's `UI_*` model, spawn
/// it looping sequence 0, resize the booth target to the window (the scene is a fullscreen render),
/// seat the character on the stage spot, and hold the booth camera on the scene's authored camera 0
/// — every frame, so the per-bake body framing from [`sync_glue_booth`] never wins while a scene
/// shows. Leaving the screen (`look: None`) tears the scene down and restores the square target.
#[allow(clippy::type_complexity)]
pub(super) fn sync_glue_scene(
    mut commands: Commands,
    preview: Res<GluePreview>,
    mut scene: ResMut<CreateScene>,
    booths: Res<Booths>,
    asset_server: Res<AssetServer>,
    m2s: Res<Assets<M2Model>>,
    booth_light: Res<BoothLight>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    mut images: ResMut<Assets<Image>>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    mut cams: Query<(&BoothCam, &mut Transform, &mut Projection)>,
    window: Query<&Window, With<PrimaryWindow>>,
    device: Res<bevy::render::renderer::RenderDevice>,
    queue: Res<bevy::render::renderer::RenderQueue>,
    // The scene's particle-emitter spawn wiring + the skin-palette table and
    // its mirror registry + the model-forms cache and mesh store
    // — one tuple param (the 16-SystemParam ceiling).
    particle_assets: (
        ResMut<benilla_world::rig_palette::RigPalettes>,
        ResMut<benilla_world::rig_palette::RigPaletteMirrors>,
        ResMut<benilla_world::model_forms::ModelForms>,
        ResMut<Assets<Mesh>>,
        ResMut<benilla_world::instance_tint::InstanceTintMirrors>,
        // …and the scene's **material-animation lane**: the UV and tint registries and the shared
        // delta table its samples land in. Here rather than beside, for the
        // same ceiling — the param list is already full.
        ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
        ResMut<benilla_world::doodad_anim::TintAnimMaterials>,
        ResMut<benilla_world::mat_anim_table::MatAnimTable>,
        ResMut<benilla_world::mat_anim_table::MatAnimMirrors>,
    ),
    // The swap gate's two inputs ([`PendingSwap`]) — the character assembly this stage must land
    // beside, and the real clock its bound is measured on. One tuple param: the 16-SystemParam
    // ceiling, same reason as `particle_assets` above.
    swap: (Res<GluePreviewBake>, Res<Time<bevy::time::Real>>),
) {
    let (
        mut palettes,
        mut mirrors,
        mut forms,
        mut mesh_assets,
        mut tint_mirrors,
        mut uv_reg,
        mut tint_reg,
        mut anim_table,
        mut anim_mirrors,
    ) = particle_assets;
    let (bake, real) = swap;
    let Some(booth) = booths.0.get(GLUE_SLOT) else {
        return;
    };
    let Some(which) = preview.scene else {
        // Off the glue screens: tear the scene down, free the model, square target back.
        if scene.token.is_some() {
            commands.entity(scene.root).despawn_related::<Children>();
            // The whole rig-state strip (pose buffer, player, palette slot — `clear_booth_rig`):
            // the scene root outlives its bakes.
            super::clear_booth_rig(&mut commands, scene.root);
            scene.token = None;
            scene.handle = None;
            scene.spawned = false;
            scene.cam = None;
            scene.art = None;
            scene.viewport_aspect = None;
            scene.char_spot = Vec3::ZERO;
            scene.char_facing = Quat::IDENTITY;
            scene.char_scale = 1.0;
            clear_pet(&mut commands, &mut scene);
            resize_target(&mut images, &booth.target, GLUE_SIZE, GLUE_SIZE);
        }
        return;
    };

    // The booth target follows the window while the screen is up — the scene is a fullscreen
    // render, so the bake wants window-native resolution (the fallback character-only render
    // shares it; its projection is aspect-aware).
    //
    // …and so does the **frame**. One box for every scene means the box is a
    // property of the window alone, so it is set here — from the window, before a token is even
    // resolved — and not down in the camera block with the scene's own fov. Two consequences, and
    // both are the point: the chrome's canvas ([`crate::glue::GlueCanvas`]) cannot move when the
    // selected character's race changes the stage, and it cannot flicker out to the full window
    // for the frames a stage swap is in flight.
    if let Ok(w) = window.single() {
        resize_target(
            &mut images,
            &booth.target,
            w.physical_width().max(1),
            w.physical_height().max(1),
        );
        let box_aspect = glue_box_aspect(w.width() / w.height().max(1.0));
        if scene.viewport_aspect != box_aspect {
            scene.viewport_aspect = box_aspect;
        }
    }

    let token = scene_token(which);
    // The fog law forks on the scene kind: the main menu is ALWAYS fogged with
    // its authored XML fog; for a race stage it is per-SCREEN — create keeps `CharModelFogInfo`,
    // select renders the same scene unfogged (`0x472110` overwrites the background's fog
    // callback). The screen is told by the look's flavor (every create
    // writer sets a Create look with the scene; select feeds Select or, on an empty account, None).
    let fog = match which {
        GlueScene::MainMenu => true,
        GlueScene::Race(_) => matches!(preview.look, Some(GlueLook::Create(_))),
    };
    // The GHOST light fork (`0x472150`, and [`ghost_rig`]): the selected
    // roster record's `CHARSELECT+0xfc & 0x2000`, which reaches the background scene, the character
    // and the pet through one `[0x83856c]`-keyed read in the reference and through one light buffer
    // here — all three of ours bind it.
    let ghost = matches!(
        preview.look,
        Some(GlueLook::Select(l)) if l.flags & benilla_protocol::CHARACTER_FLAG_GHOST != 0
    );
    if scene.token == Some(token) {
        // The request came back to the stage already standing (a re-click, a same-race row) —
        // whatever swap was in flight is moot.
        scene.pending = None;
    }
    if scene.token != Some(token) {
        // **The pair swaps together, or not at all** — see [`PendingSwap`] for the defect this
        // exists to close and the measurements behind it. The load starts on the click; only the
        // commit waits, and only while there is a coherent pair on the stage to protect.
        let now = real.elapsed_secs();
        let pending = match scene.pending.take() {
            Some(p) if p.token == token => p,
            _ => PendingSwap {
                token,
                handle: asset_server.load(m2_url(&format!(
                    "Interface\\Glues\\Models\\UI_{token}\\UI_{token}.mdx"
                ))),
                since: now,
            },
        };
        // Both halves of the REQUESTED selection: this stage's art, and the assembly for the look
        // that asked for it (`build_glue_preview` publishes the look it actually built, so this
        // compares "what stands" against "what was clicked" without re-deriving either).
        let art_ready = m2s.get(&pending.handle).is_some();
        let body_ready = bake.look == preview.look;
        // An empty stage has no pair to break — the login → select hop, an empty account, a
        // capture booting onto a glue screen.
        let standing = bake.look.is_some() && !bake.parts.is_empty();
        let waited = now - pending.since;
        if !may_swap(standing, art_ready, body_ready, waited) {
            scene.pending = Some(pending);
            return; // hold the coherent pair already on the stage
        }
        if standing && waited >= GLUE_SWAP_HOLD_SECS {
            warn!(
                "glue scene: UI_{token} swapping after {waited:.1}s without its pair (art \
                 {art_ready}, body {body_ready}) — the bound is GLUE_SWAP_HOLD_SECS"
            );
        }
        scene.token = Some(token);
        scene.handle = Some(pending.handle);
        scene.spawned = false;
        scene.cam = None;
        scene.art = None;
        // `viewport_aspect` is deliberately NOT cleared: the frame is the window's, not this
        // stage's (2187), and clearing it here is what made the chrome jump out to the full
        // window for the frames between two stages.
        scene.char_spot = Vec3::ZERO;
        clear_pet(&mut commands, &mut scene);
        commands.entity(scene.root).despawn_related::<Children>();
        // The model may still be loading — strip the old rig state off the outliving root now,
        // not only at the next spawn (which may be many frames away).
        super::clear_booth_rig(&mut commands, scene.root);
    } else if scene.spawned && scene.fog != fog {
        // Same scene, other screen: rebuild in place (the handle is cached — same-frame respawn).
        scene.spawned = false;
        commands.entity(scene.root).despawn_related::<Children>();
    } else if scene.spawned && scene.ghost != ghost {
        // Same scene, same screen, the selection crossed the ghost bit: **re-light, don't rebuild**.
        // The reference's fork is a per-frame fill callback on models that are already standing
        // (`0x472150` on `[model+0x3bc]`), so nothing is respawned there either — and respawning
        // here would restart the stage's own emitters, which the click has no business touching.
        if let (Some(model), Some(light)) = (
            scene.handle.as_ref().and_then(|h| m2s.get(h)),
            scene.light.clone(),
        ) {
            let rig = if ghost {
                ghost_rig()
            } else {
                scene_rig(&model.lights)
            };
            let (fog_rgb, fog_far) = scene_fog(token);
            scene_light_blob(&rig, fog_rgb, fog_far, fog).write(&queue, &light);
            scene.ghost = ghost;
            // The character and the pet re-bake onto the rewritten buffer (`sync_glue_booth` and
            // `sync_glue_pet` both key on this).
            scene.rev += 1;
            info!(
                "glue scene: {} light — {} directional, {} point light(s)",
                if ghost { "GHOST" } else { "authored" },
                rig.lobes.len(),
                rig.points.len(),
            );
        }
    }
    if !scene.spawned {
        let Some(model) = scene.handle.as_ref().and_then(|h| m2s.get(h)) else {
            return; // still loading (the flat page shows in the meantime)
        };
        if booth_light.studio.buffer.is_none() {
            return; // headless — no booth pipeline, no scene
        }
        // The scene's light: its **authored M2 rig** (ambient + the strongest key toward the
        // stage) plus the ref's per-race fog (`CharModelFogInfo`) — its own buffer, so the
        // portraits' shared studio light stays untouched. Rewritten in place on every scene swap
        // (the cached scene + character materials keep binding it).
        let stage = attachment_point(&model.skeleton, &model.attachments, 0).unwrap_or(Vec3::ZERO);
        // The stage's frame, not just its point — see [`stage_frame`].
        let (stage_facing, stage_scale) = model
            .attachments
            .iter()
            .find(|a| a.id == 0)
            .map_or((Quat::IDENTITY, 1.0), |a| stage_frame(model, a.bone));
        // A GHOST selection replaces the gather outright (see [`ghost_rig`]); a living one keeps
        // the scene's authored rig, which is the create law select-confirmed.
        let rig = if ghost {
            ghost_rig()
        } else {
            scene_rig(&model.lights)
        };
        let (fog_rgb, fog_far) = scene_fog(token);
        let blob = scene_light_blob(&rig, fog_rgb, fog_far, fog);
        let light = scene
            .light
            .get_or_insert_with(|| blob.create(&device, "wow_create_scene_light"))
            .clone();
        // The scene's rigs skin from THIS buffer's palette region: register it
        // as a mirror (re-registration replaces — a rebuilt scene's old buffer drops).
        mirrors.0.insert("glue_scene", light.clone());
        // …and the per-instance TINT region beside the palette rows (decision 0812's channel):
        // the glue character's own modulate colour is read out of THIS buffer, and the glue scene
        // is the one off-world buffer that carries it — see
        // [`benilla_world::instance_tint::InstanceTintMirrors`] for why the portrait booths do not.
        tint_mirrors.0.insert("glue_scene", light.clone());
        // …and the **mat-anim delta table** beside them: the scene's
        // materials sample `matanim[slot]` out of THIS buffer, so registering a batch's loop is
        // only half the act — without the mirror the rows are written every frame into a region
        // only the world's materials ever read, and the scene samples its zero-initialised
        // identity. That is verbatim 2023's cooldown-sweep failure, and it is what an A/B of this
        // screen with the lane frozen against the lane running showed: byte-identical images,
        // with 2 of UI_Human's 29 batches registered and marked.
        //
        // The portrait booths stay off this list on purpose ([`MatAnimMirrors`]'s own note): a
        // bake photographs one instant, and its studio buffer's zeroed region IS that instant.
        // The create screen is not a bake.
        anim_mirrors.0.insert("glue_scene", light.clone());
        blob.write(&queue, &light);
        scene.fog = fog;
        scene.ghost = ghost;
        let dc = blob.probe_dc();
        info!(
            "create scene: UI_{token} rig — ambient {:?}, {} point light(s), probe dc ({:.2},{:.2},{:.2})",
            rig.ambient,
            blob.point_count(),
            dc[0],
            dc[1],
            dc[2],
        );
        // The scene model's render forms, built NOW: one backdrop model, behind
        // the glue screen — the booth-lane exception to the paced rule.
        let scene_handle = scene
            .handle
            .as_ref()
            .expect("scene.handle is Some — `model` above came from it");
        forms.ensure_now_rigged(scene_handle, &model.submeshes, &mut mesh_assets);
        let built = forms.slices(scene_handle);
        let (stat_forms, skin_forms) = (built.stat, built.skin.unwrap_or(&[]));
        let scene_parts: Vec<BoothPart> = model
            .submeshes
            .iter()
            .enumerate()
            .map(|(pi, s)| {
                // The create scene is lit by its OWN authored M2 rig against its own buffer —
                // never the world's sun — and its clouds and water scroll.
                //
                // The authored batch order (index + 1) rides with it: a scene's
                // batches all hang off the scene root, so they share one transparent sort distance
                // and only this bias tells them apart — without it `UI_Human`'s ground-shadow decal
                // and its street tied, and the tie re-broke every frame.
                let order = u16::try_from(pi + 1).unwrap_or(u16::MAX);
                let material = mats.off_world(s, s.texture.clone(), order, &light, true);
                // **The scene's texture transforms and M2Color tint actually run** — the clouds
                // drift, the water and lava creep, the fire sheets scroll. `off_world(rig: true)`
                // has always *seeded* the UV at `sample(0.0)`, but seeding is not playing: nothing
                // outside the world streamer, the entity dressing path and the effect lane had
                // ever been registered, so every one of these froze on its first key while the
                // comment above claimed they scrolled.
                //
                // Not a bake question, which is why it is decided here rather than deferred to
                // 0130's bake law: a create/main-menu screen is a LIVE render, and the reference
                // runs the texture transform inside the per-model-per-frame animate kernel
                // (`0x715f25`-`0x7163bc` over the MD20 `+0x74` table) — it is what an animated
                // CM2Model does, not a lane a host opts into. The portrait and dressing-room bakes
                // are the opposite case and stay still (`BoothPart::mat_anim`).
                //
                // **Shared lane only, and that is measured, not assumed** (`benilla-extract
                // uvslotscan interface\glues`): 13 models, 261 batches, **15 live UV loops over 6
                // models and 2 live RGB**, and PER-PLACEMENT is empty on both channels — every
                // glue batch bakes one loop across its slots, so there is no sequence to key by
                // and `host: None` is exact rather than a fallback.
                let loops = benilla_world::doodad_anim::UvLoops::of(s);
                let mut mat_anim = false;
                if loops.animates() {
                    benilla_world::doodad_anim::register_entity_uv(
                        &mut uv_reg,
                        &mut anim_table,
                        mats.materials(),
                        material.id(),
                        &loops,
                        None,
                    );
                    mat_anim = true;
                }
                if let Some(rgb) = s.rgb_anim.as_ref().filter(|a| a.period > 0.0) {
                    benilla_world::doodad_anim::register_tint(
                        &mut tint_reg,
                        &mut anim_table,
                        mats.materials(),
                        material.id(),
                        benilla_world::doodad_anim::TintLoop::Shared(rgb.clone()),
                    );
                    mat_anim = true;
                }
                BoothPart {
                    skinned: skin_forms.get(pi).cloned(),
                    static_mesh: stat_forms
                        .get(pi)
                        .map(|(h, _)| h.clone())
                        .unwrap_or_default(),
                    material,
                    // The scene's authored per-batch material alpha. UI_Tauren
                    // alone: a 0.55 `LENSALPHA` corner vignette, a 0.99 `GROUNDSHADOW` decal, 0.15
                    // `CLOUDS`, a 0.73 gradient. Drawn at 1.0 the vignette blacked out the frame
                    // corners — B121.
                    alpha_anim: s.alpha_anim.clone(),
                    twins: BoothTwins::default(),
                    mat_anim,
                }
            })
            .collect();
        info!(
            "create scene: UI_{token} material lane — {} of {} batches animate (UV/tint registered)",
            scene_parts.iter().filter(|p| p.mat_anim).count(),
            scene_parts.len(),
        );
        let mut scene_rig = spawn_booth_model(
            &mut commands,
            &mut palettes,
            scene.root,
            booth.layer.clone(),
            &scene_parts,
            &[],
            Some((
                &model.skeleton,
                &model.inverse_bindposes,
                model.animations.as_ref(),
            )),
            anim_data.as_deref().map(|a| &a.0),
            BoothMotion::Loop, // sequence 0 loops — flags wave, clouds drift
            [false, false],    // the backdrop scene has no hands to grip
            &[],               // …nor a character's eye-glow
            BoothInstance::default(),
        );
        // The scene's authored particle emitters — the braziers/embers every
        // UI_* scene carries (MainMenu 28, Orc 11, NightElf 12…). Lit/fogged by the SCENE's own
        // light buffer: the ModelFFX fog covers the whole model, emitters included, carried by
        // `EffectLightOverride` into the shared lane's per-draw bind group. The seating, the
        // billboard arm and the teardown parenting are the shared recipe's (1539).
        let (emitters, fx_frames) = super::spawn_booth_own_emitters(
            &mut commands,
            &mut scene_rig,
            scene.root,
            &booth.layer,
            Some(&light),
            &model.emitters,
        );
        if emitters > 0 {
            info!(
                "create scene: UI_{token} — {emitters} particle emitter(s) up \
                 ({fx_frames} on a billboard frame)"
            );
        }
        scene_rig.finish(&mut commands);
        scene.spawned = true;
        scene.cam = model.camera0;
        // The scene's art extent — the shipped table's transcription of what the tool measured
        // off this very model. Said out loud as the aspects the law acts on.
        scene.art = benilla_formats::shipped_glue_art_extent(token);
        if let (Some(art), Some(cam)) = (scene.art, model.camera0.as_ref()) {
            let t0 = benilla_formats::authored_half_height(cam.fov);
            // Where the frame's edge lands on THIS stage's art (2187): the constant is derived
            // from the seven shipped scenes, so on the shipped chain this reads "inside" for six
            // of them and a hair past the night elves' 4:3 sky card. A patched or replaced scene
            // that paints narrower would say so here.
            let frame = super::framing::GLUE_BOX_ASPECT * super::framing::glue_zoom_floor(cam.fov);
            info!(
                "create scene: UI_{token} art extent — half_w {:.4} (widens to {:.2}:1), half_h {:.4} \
                 (opens to 1:{:.2}); the frame ends at {frame:.4} — {}",
                art.half_w,
                art.half_w / t0,
                art.half_h,
                art.half_h / (t0 * GLUE_AUTHORED_ASPECT),
                if frame <= art.half_w.max(t0 * GLUE_AUTHORED_ASPECT) {
                    "inside the art"
                } else {
                    "PAST the art (void at the frame's edges)"
                },
            );
        }
        scene.char_spot = stage;
        scene.char_facing = stage_facing;
        scene.char_scale = stage_scale;
        // The pet's seat comes off the same skeleton, one attachment along — frame and all. A scene
        // that somehow authors none leaves the pet at the origin rather than on the stage — visibly
        // wrong, and it never happens: all six `UI_*` scenes author 0 and 1.
        scene.pet_spot =
            attachment_point(&model.skeleton, &model.attachments, 1).unwrap_or(Vec3::ZERO);
        let (pet_facing, pet_stage_scale) = model
            .attachments
            .iter()
            .find(|a| a.id == 1)
            .map_or((Quat::IDENTITY, 1.0), |a| stage_frame(model, a.bone));
        scene.pet_facing = pet_facing;
        scene.pet_stage_scale = pet_stage_scale;
        // A rebuilt scene moved the seat, so the standing pet is re-seated (its bake is still good;
        // `sync_glue_pet` re-runs the placement every frame the spot changes).
        scene.pet_baked = None;
        scene.rev += 1; // the character re-lights onto the scene rig (sync_glue_booth's key)
        info!(
            "create scene: UI_{token} up — {} submeshes, camera {} stage {:?} facing {:.2}° \
             scale {:.3}",
            model.submeshes.len(),
            scene.cam.is_some(),
            scene.char_spot,
            scene.char_facing.to_euler(EulerRot::YXZ).0.to_degrees(),
            scene.char_scale,
        );
        // Seat the character on the stage at the current facing (the yaw-only path below only
        // runs when the yaw *changes*).
        apply_yaw(
            &mut commands,
            booth.root,
            scene.char_spot,
            scene.char_facing,
            scene.char_scale,
            preview.yaw,
        );
    }
    // The authored camera owns the booth camera while the scene shows (re-asserted every frame —
    // cheap, and it also tracks live window-aspect changes through the diagonal-FOV conversion).
    if let Some(cam) = scene.cam {
        let aspect = window
            .single()
            .map(|w| w.width() / w.height().max(1.0))
            .unwrap_or(4.0 / 3.0);
        let fwd = (cam.target - cam.eye).normalize_or_zero();
        let up = Quat::from_axis_angle(fwd, cam.roll) * Vec3::Y;
        // The record fov is the client's *diagonal* convention. The scene is a FULL-SCREEN pane, so
        // the reference feeds that conversion the display's own aspect and spends height on width —
        // a 1.55× zoom at 21:9 that crops head and feet. [`glue_scene_framing`] pins the
        // authored 4:3 view box instead; its doc carries the law. Far kept generous: fog is not
        // rendered yet, so the authored far (27.8 on Orc) would slice unfogged geometry.
        let vert_fov = glue_scene_framing(cam.fov, aspect, scene.art);
        let rig = (
            Transform::from_translation(cam.eye).looking_at(cam.target, up),
            Projection::from(PerspectiveProjection {
                fov: vert_fov,
                near: cam.near,
                far: cam.far.max(1000.0),
                ..default()
            }),
        );
        aim(&mut cams, GLUE_SLOT, &rig);
    }
}

/// Apply the framing law's **pillarbox** to the glue booth camera: while
/// [`CreateScene::viewport_aspect`] is set, the camera renders into a centred viewport of that
/// aspect on the window-sized target, and the target either side is the camera's black *output*
/// clear (see [`spawn_glue_booth`]) — a frame. Inside the box nothing changes: the scene's own
/// unpainted pixels stay transparent over the page, as they always were. Writes only on change —
/// a `Camera` write re-runs the camera system.
pub(super) fn pillarbox_glue_scene(
    scene: Res<CreateScene>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut cams: Query<(&BoothCam, &mut Camera)>,
) {
    let Ok(w) = window.single() else {
        return;
    };
    let full_h = w.physical_height().max(1);
    // The SAME arithmetic the chrome's canvas insets by ([`glue_box_physical`]) —
    // one function, so the frame the camera renders and the frame the chrome lays out in cannot
    // round apart.
    let viewport =
        glue_box_physical(w.physical_width(), full_h, scene.viewport_aspect).map(|(x, box_w)| {
            bevy::camera::Viewport {
                physical_position: UVec2::new(x, 0),
                physical_size: UVec2::new(box_w, full_h),
                ..default()
            }
        });
    for (booth, mut cam) in cams.iter_mut() {
        if booth.0 != GLUE_SLOT {
            continue;
        }
        let same = match (&cam.viewport, &viewport) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.physical_position == b.physical_position && a.physical_size == b.physical_size
            }
            _ => false,
        };
        if !same {
            cam.viewport = viewport.clone();
        }
    }
}

/// Resize the booth's render target if it isn't already `w`×`h` (content is discarded — the next
/// frame repaints it whole).
fn resize_target(images: &mut Assets<Image>, target: &Handle<Image>, w: u32, h: u32) {
    // Through the house write-gate ([`benilla_assets::write_gated`]), for the reason that helper
    // exists: `Assets::get_mut` queues `AssetEvent::Modified` unconditionally — it cannot know
    // whether the caller wrote anything — and a Modified image is re-extracted and re-uploaded
    // whole by `prepare_assets<GpuImage>` next frame. This target is the glue scene's FULLSCREEN
    // render target: at 3200×1800 it is 22 MB, and taking the borrow once per frame just to
    // compare two u32s re-uploaded all 22 MB every frame on a screen where nothing moves
    // (~1.3 GB/s; measured with `WOW_ASSET_CHURN=1`). This was the ONE asset write
    // site in the tree not already behind that gate.
    benilla_assets::write_gated(
        images,
        target,
        |image| image.width() != w || image.height() != h,
        |image| {
            image.resize(Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            });
        },
    );
}

/// Re-bake the glue booth when the assembled parts change (a dial moved, a different character
/// selected, an item model landed) or the scene's light rig lands, and spin its root to the
/// screen's live yaw (drag / rotate — cheap, no re-bake). Mirrors [`super::sync_paperdoll`]: the
/// parts + riders come pre-assembled from the entities builder, so here we only re-light them —
/// onto the **scene's authored rig** while a scene is up (the ref's glue character is scene-lit;
/// the select screen's hardcoded weather-sun is not yet settled),
/// the studio buffer otherwise — pose a fresh Stand-**looping** instance with the riders on its
/// joints, and frame it full-body.
pub(super) fn sync_glue_booth(
    mut commands: Commands,
    preview: Res<GluePreview>,
    bake: Res<GluePreviewBake>,
    mut booths: ResMut<Booths>,
    mut scene: Option<ResMut<CreateScene>>,
    mut booth_light: ResMut<BoothLight>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
    creatures: Option<Res<Creatures>>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    mut cams: Query<(&BoothCam, &mut Transform, &mut Projection)>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    // The per-instance modulate colour: a ghost selection paints the whole bake
    // through it, keyed on the rig slot this bake allocates — the reference's `cc+0x184..0x18c`.
    mut tints: ResMut<benilla_world::instance_tint::InstanceTints>,
    mut last: Local<Option<(u64, f32, u64)>>,
) {
    let Some(booth) = booths.0.get_mut(GLUE_SLOT) else {
        return;
    };
    // Where the character stands (the scene's stage spot while a scene is up, else the origin) —
    // and the scene revision, so the bake re-lights the moment a scene rig lands.
    let (spot, stage, stage_scale, scene_rev) =
        scene
            .as_deref()
            .map_or((Vec3::ZERO, Quat::IDENTITY, 1.0, 0), |s| {
                if s.spawned {
                    (s.char_spot, s.char_facing, s.char_scale, s.rev)
                } else {
                    (Vec3::ZERO, Quat::IDENTITY, 1.0, s.rev)
                }
            });
    let (last_rev, last_yaw, last_scene) = last.unwrap_or((u64::MAX, f32::NAN, u64::MAX));
    // **The other half of [`PendingSwap`]'s law**, for the arrival order it cannot catch: a body
    // whose models are already cached assembles in a frame, under a stage whose art is still
    // loading — measured at 34 ms of a human standing in the orc stage. `sync_glue_scene` runs
    // ahead of this system and commits both halves in the frame they are both ready, so "the
    // standing stage is the requested one" is exactly "this bake may be shown".
    let staged = scene
        .as_deref()
        .is_none_or(|s| s.token == preview.scene.map(scene_token));
    let rebake = staged && (last_rev != bake.revision || last_scene != scene_rev);
    let reyaw = last_yaw != preview.yaw;
    if !rebake && !reyaw {
        return;
    }

    if rebake {
        // Empty parts (nothing selected, or the model failed) → clear the booth.
        if bake.parts.is_empty() {
            commands.entity(booth.root).despawn_related::<Children>();
            // The despawn reaped meshes and anchors; the rig state on the ROOT needs its own
            // strip ([`super::clear_booth_rig`]).
            super::clear_booth_rig(&mut commands, booth.root);
            booth.rigged = false;
            booth.parked = false;
            *last = Some((bake.revision, preview.yaw, scene_rev));
            apply_yaw(
                &mut commands,
                booth.root,
                spot,
                stage,
                stage_scale,
                preview.yaw,
            );
            return;
        }
        // The rig + framing come from the display cache — the same readiness gate the builder used
        // to produce the parts, so both are ready together. If somehow not, leave `last` so we retry.
        let Some(creatures) = creatures.as_deref() else {
            return;
        };
        let (Some(rig), Some(anchors)) = (
            creatures.display_rig(bake.display_id),
            creatures.display_anchors(bake.display_id),
        ) else {
            return;
        };
        let scene_buf = scene
            .as_deref()
            .and_then(|s| if s.spawned { s.light.clone() } else { None });
        let relight = |material: &Handle<WowModelMaterial>,
                       scene: &mut Option<ResMut<CreateScene>>,
                       booth_light: &mut BoothLight,
                       materials: &mut Assets<WowModelMaterial>| {
            match (&scene_buf, scene.as_deref_mut()) {
                // The glue screens keep the old fallback deliberately: the create/select scene has
                // its own lifecycle (no map-scope teardown under it) and no parts-key retry to
                // ride, so waiting here would leave the pane empty instead of merely mislit.
                (Some(buf), Some(s)) => super::material_variant(
                    &mut s.variants,
                    buf,
                    material,
                    materials,
                    VariantLane::RigUnfogged,
                )
                .unwrap_or_else(|| material.clone()),
                _ => booth_light.studio.variant(material, materials),
            }
        };
        // The bake's instance-level render properties — the ghost kit's alpha, or opaque
        // (`GhostKit`, `0x4727f0`). It reaches every batch of the bake, the
        // riders and cards included: the reference has ONE instance alpha per CM2 and every model
        // attached to it composes onto that (`0x714000`).
        let instance = BoothInstance {
            alpha: bake.ghost.map_or(1.0, |g| g.alpha),
        };
        // Each twin is relit onto the same buffer as its steady sibling — a blend twin bound to the
        // world's light buffer would draw the ghost body at world-sun intensity mid-feather.
        let relight_twins = |t: &BoothTwins,
                             scene: &mut Option<ResMut<CreateScene>>,
                             booth_light: &mut BoothLight,
                             materials: &mut Assets<WowModelMaterial>| {
            if !instance.feathering() {
                return BoothTwins::default(); // an opaque bake never reads them
            }
            BoothTwins {
                blend: t
                    .blend
                    .as_ref()
                    .map(|m| relight(m, scene, booth_light, materials)),
                zfill: t
                    .zfill
                    .as_ref()
                    .map(|m| relight(m, scene, booth_light, materials)),
            }
        };
        let mut booth_parts = Vec::with_capacity(bake.parts.len());
        for p in &bake.parts {
            booth_parts.push(BoothPart {
                skinned: p.skinned_mesh.clone(),
                static_mesh: p.static_mesh.clone(),
                material: relight(&p.material, &mut scene, &mut booth_light, &mut materials),
                twins: relight_twins(&p.twins, &mut scene, &mut booth_light, &mut materials),
                // `None`, and a KNOWN GAP: a mirrored `PreviewPart` doesn't carry
                // the batch's alpha loops, so a character batch with an authored dimming constant
                // previews at 1.0 here. Closing it means threading `alpha_anim` through the
                // appearance-assembly layer — deliberately not folded into the B121 fix.
                alpha_anim: None,
                mat_anim: false,
            });
        }
        // The equipment riders (a Select look — helm/shoulders/sheathed weapons):
        // scene-lit like the body, seated on the throwaway skeleton's joints by the booth spawn.
        let booth_riders: Vec<BoothRider> = bake
            .riders
            .iter()
            .map(|r| BoothRider {
                mesh: r.mesh.clone(),
                material: relight(&r.material, &mut scene, &mut booth_light, &mut materials),
                bone: r.bone,
                offset: r.offset,
                twins: relight_twins(&r.twins, &mut scene, &mut booth_light, &mut materials),
            })
            .collect();
        // The camera-facing batches (the eye-glow, a wand's gem, a glow model's quad), relit onto the
        // booth's buffer like everything else (harmless where the batch is fullbright — the variant
        // just keeps parity).
        let booth_billboards: Vec<BoothBillboardSpec> = bake
            .billboards
            .iter()
            .map(|b| BoothBillboardSpec {
                mesh: b.mesh.clone(),
                material: relight(&b.material, &mut scene, &mut booth_light, &mut materials),
                bone: b.bone,
                offset: b.offset,
                kind: b.kind,
                twins: relight_twins(&b.twins, &mut scene, &mut booth_light, &mut materials),
            })
            .collect();
        // `WOW_BOOTH_LOG=1` — **which body is standing, and what put it there.** The gate timeline
        // says the glue camera drew; only this says whose display it drew, and whether the re-bake
        // was the look changing (`look`) or the SCENE's rig landing under an unchanged look
        // (`scene`) — the second of which stands the OUTGOING character in the INCOMING race's
        // stage until the new assembly is ready.
        if super::booth_log() {
            println!(
                "[booth] glue rebake display={} parts={} riders={} bake_rev={} scene_rev={scene_rev} by={}",
                bake.display_id,
                bake.parts.len(),
                bake.riders.len(),
                bake.revision,
                if last_rev != bake.revision { "look" } else { "scene" },
            );
        }
        commands.entity(booth.root).despawn_related::<Children>();
        let mut booth_rig = spawn_booth_model(
            &mut commands,
            &mut palettes,
            booth.root,
            booth.layer.clone(),
            &booth_parts,
            &booth_riders,
            rig.inverse_bindposes
                .as_ref()
                .map(|ibp| (rig.skeleton, ibp, rig.animations)),
            anim_data.as_deref().map(|a| &a.0),
            BoothMotion::Loop, // the glue preview is a live scene, not a still
            bake.grip,         // close each hand that draws a weapon (the ref's paperdoll rule)
            &booth_billboards,
            instance,
        );
        // The whole-model TINT (`GhostKit::tint` — the reference's `cc+0x184..0x18c`, written by the
        // select ghost proc `0x472939 -> 0x710cf0`): the bake's own rig slot indexes the per-instance
        // table, and every batch that carries that slot in its `MeshTag` — body, riders, cards —
        // reads it. Written every re-bake, cleared for a living selection, so a ghost's colour can
        // never survive into the next character you click. (Slot `0` = a boneless bake or a full
        // palette; `set` refuses it, which is the world's own no-rig sentinel rule.)
        tints.set(
            booth_rig.slot(),
            match bake.ghost {
                Some(g) => benilla_world::instance_tint::pack(g.tint),
                None => benilla_world::instance_tint::IDENTITY,
            },
        );
        // The worn items' effects: an equipped item model's OWN emitters (the R14
        // pauldron's sparkle, the torch's flame) and the held weapons' `ItemVisuals`
        // glows. One host per effect model at its seat, emitters owned by it — the
        // scene-brazier recipe, so they draw against THIS camera and fog on the scene's own
        // light. Children of the booth root through their joint, so the next re-bake's
        // `despawn_related` reaps them. The reference reaches both through the very same attach
        // primitive it uses in the world (`0x472c91` → `0x47a0c0` → `0x4798c0`), and the enum carries
        // no enchant, so what shows here is each item's intrinsic effects only.
        let (fx_emitters, fx_frames) = spawn_booth_effects(
            &mut commands,
            &mut booth_rig,
            &booth.layer,
            scene_buf.as_ref(),
            &bake
                .effects
                .iter()
                .map(|fx| BoothEffects {
                    bone: fx.bone,
                    offset: fx.offset,
                    emitters: fx.emitters.clone(),
                })
                .collect::<Vec<_>>(),
            instance,
        );
        if fx_emitters > 0 {
            info!(
                "glue booth: {fx_emitters} item emitter(s) up on {} effect model(s), \
                 {fx_frames} on a billboard frame",
                bake.effects.len()
            );
        }
        // A fresh bake is animated by construction; the park state is the new rig's.
        booth.rigged = booth_rig.rigged();
        booth_rig.finish(&mut commands);
        booth.parked = false;
        // The character-only fallback framing (no scene up — art missing / still loading): the
        // body-frame transform with an aspect-aware projection, because the booth target is
        // window-sized now (the square-true `WowPortraitProjection` would stretch on it). While a
        // scene shows, `sync_glue_scene` re-aims onto the authored camera the same frame.
        // Aspect 1.0 into `body_frame`: only its TRANSFORM is kept here — the projection
        // beside it is the window-sized one this booth needs (decision 1069's aspect rides on
        // the discarded half).
        //
        // Since 1089 that transform is the model's own `<PlayerModel>` pane camera (raw index 1),
        // which is the client's full-body character framing — the right stand-in, and the fov below
        // now comes off the same record rather than from the retired body-fit constant. The crop
        // aspect is the portrait bake's ([`PORTRAIT_ASPECT`]) rather than this window's: exact
        // framing here does not matter, because `sync_glue_scene` re-aims onto the scene's authored
        // camera the frame the art lands, and this rig only ever shows for those few loading
        // frames.
        // …and ONLY with no scene up. It always was "the fallback" in intent (a scene re-aims
        // onto its authored camera), but it used to rely on `sync_glue_scene` running *after* this
        // system to overwrite it — which is also what made the character read last frame's stage
        // and gave [`PendingSwap`] its one-frame gap. With the order corrected, the fallback has
        // to know it is one.
        let (t, _) = body_frame(&anchors, 1.0);
        let record_fov = anchors
            .pane_camera
            .map_or(super::framing::PANE_FIXED_FOV, |c| c.fov);
        if !scene.as_deref().is_some_and(|s| s.spawned) {
            aim(
                &mut cams,
                GLUE_SLOT,
                &(
                    t,
                    Projection::from(PerspectiveProjection {
                        fov: diag_to_vert(record_fov, PORTRAIT_ASPECT),
                        near: 0.02,
                        far: 100.0,
                        ..default()
                    }),
                ),
            );
        }
    }
    apply_yaw(
        &mut commands,
        booth.root,
        spot,
        stage,
        stage_scale,
        preview.yaw,
    );
    // A yaw-only pass must not record the bake/scene revisions it did NOT commit, or the held
    // pair above would never be re-tried once the stage lands.
    *last = Some((
        if rebake { bake.revision } else { last_rev },
        preview.yaw,
        if rebake { scene_rev } else { last_scene },
    ));
}

/// Stand the select screen's **pet** on scene attachment 1 — the hunter/warlock companion beside
/// the selected character (the reference's secondary model `record+0x114`, `0x47306b`).
///
/// The booth twin of [`sync_glue_booth`], and deliberately the simpler one: a pet is an ordinary
/// creature display, so its parts come straight off the shared display cache with no compositor,
/// no equipment, no geoset selection and no riders. What it does share is the law that matters —
/// a fresh instance looping its own **Stand**, relit onto the scene's authored rig, on the booth's
/// layer — plus the model's own particle emitters riding its own joints, which is what makes an
/// imp an imp.
///
/// It runs only while a scene is up: both the seat and the light come from the background model,
/// and a pet standing at the origin next to a body standing at the origin is worse than no pet at
/// all for the frame or two the art takes to land.
///
/// **Not yaw-driven.** Dragging the character spins the character: the reference's facing call
/// (`0x4730e0`) writes the character component's model transform and nothing else, and both scene
/// attachments are parentless unkeyed pivots, so the pet keeps the seat's own orientation.
pub(super) fn sync_glue_pet(
    mut commands: Commands,
    pet: Res<GluePetBake>,
    booths: Res<Booths>,
    mut scene: Option<ResMut<CreateScene>>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
    creatures: Option<Res<Creatures>>,
    anim_data: Option<Res<crate::creature_anim::AnimData>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
) {
    let Some(scene) = scene.as_deref_mut() else {
        return;
    };
    let Some(booth) = booths.0.get(GLUE_SLOT) else {
        return;
    };
    // No pet on this character (every class but a living hunter/warlock) — clear once and stop.
    if pet.display_id == 0 || pet.parts.is_empty() {
        if scene.pet_baked.is_some() {
            clear_pet(&mut commands, scene);
        }
        return;
    }
    if !scene.spawned {
        return; // no stage yet — nothing to stand on, nothing to be lit by
    }
    if scene.pet_baked == Some(pet.revision) {
        return; // already standing, and nothing about it changed
    }
    // The rig comes from the same display cache that produced the parts, so both are ready
    // together; if somehow not, leave `pet_baked` alone and retry next frame.
    let (Some(creatures), Some(light)) = (creatures.as_deref(), scene.light.clone()) else {
        return;
    };
    let Some(rig) = creatures.display_rig(pet.display_id) else {
        return;
    };
    let mut relight = |material: &Handle<WowModelMaterial>,
                       variants: &mut std::collections::HashMap<
        AssetId<WowModelMaterial>,
        Handle<WowModelMaterial>,
    >| {
        super::material_variant(
            variants,
            &light,
            material,
            &mut materials,
            VariantLane::RigUnfogged,
        )
        .unwrap_or_else(|| material.clone())
    };
    let booth_parts: Vec<BoothPart> = pet
        .parts
        .iter()
        .map(|p| BoothPart {
            skinned: p.skinned_mesh.clone(),
            static_mesh: p.static_mesh.clone(),
            material: relight(&p.material, &mut scene.variants),
            alpha_anim: p.alpha_anim.clone(),
            twins: BoothTwins::default(),
            mat_anim: false,
        })
        .collect();
    let booth_billboards: Vec<BoothBillboardSpec> = pet
        .billboards
        .iter()
        .map(|b| BoothBillboardSpec {
            mesh: b.mesh.clone(),
            material: relight(&b.material, &mut scene.variants),
            bone: b.bone,
            offset: b.offset,
            kind: b.kind,
            twins: BoothTwins::default(),
        })
        .collect();
    commands
        .entity(scene.pet_root)
        .despawn_related::<Children>();
    super::clear_booth_rig(&mut commands, scene.pet_root);
    let mut pet_rig = spawn_booth_model(
        &mut commands,
        &mut palettes,
        scene.pet_root,
        booth.layer.clone(),
        &booth_parts,
        &[], // a pet wears nothing
        rig.inverse_bindposes
            .as_ref()
            .map(|ibp| (rig.skeleton, ibp, rig.animations)),
        anim_data.as_deref().map(|a| &a.0),
        BoothMotion::Loop, // Stand, looping — the same live scene the character is
        [false, false],    // …and no hands to grip with
        &booth_billboards,
        BoothInstance::default(),
    );
    // The pet's OWN emitters — the imp's flames. Authored on the pet's own bones,
    // so this is the model's-own recipe, NOT the worn-item one (`spawn_booth_effects`, which seats
    // a *separate* model's emitters on a body bone through a host): the flame at the hand rides the
    // hand through the Stand loop.
    //
    // Anchored at `pet_root`, whose scale is the pet's — a half-size imp gets half-size flames,
    // positions and quads alike (`particles::quads`, the `sizeByInstanceScale` flag all three of
    // the imp's set). Relit like every other booth draw, onto the SCENE's light buffer, so the
    // flames take the backdrop's fog instead of the world's time of day.
    let (emitters, fx_frames) = super::spawn_booth_own_emitters(
        &mut commands,
        &mut pet_rig,
        scene.pet_root,
        &booth.layer,
        Some(&light),
        &pet.emitters,
    );
    // After the emitter spawn: `anchor` writes into the pose buffer, `finish` is what commits it.
    pet_rig.finish(&mut commands);
    // The seat: attachment 1's whole frame — its translation, the constant yaw its bone carries on
    // the Human scene and the constant scale it carries on Human and Tauren (1533) — times the DBC
    // scale product the wire never carries here (see [`GluePetBake::scale`]).
    commands.entity(scene.pet_root).insert(
        Transform::from_translation(scene.pet_spot)
            .with_rotation(scene.pet_facing)
            .with_scale(Vec3::splat(pet.scale.max(0.01) * scene.pet_stage_scale)),
    );
    scene.pet_baked = Some(pet.revision);
    info!(
        "glue pet: display {} up — {} part(s), {} camera-facing, {} emitter(s) ({} on a billboard \
         frame), scale {:.3} at {:?}",
        pet.display_id,
        booth_parts.len(),
        booth_billboards.len(),
        emitters,
        fx_frames,
        pet.scale,
        scene.pet_spot,
    );
}

/// Tear the standing pet down (no pet on this character, or the stage went away).
fn clear_pet(commands: &mut Commands, scene: &mut CreateScene) {
    commands
        .entity(scene.pet_root)
        .despawn_related::<Children>();
    // The rig state lives on the root, which outlives its bakes — the despawn doesn't reach it.
    super::clear_booth_rig(commands, scene.pet_root);
    scene.pet_baked = None;
}

/// Seat the model root on the stage `spot`, in the stage's own frame (`stage` + `stage_scale`),
/// facing `yaw` (the ref's `Model:SetRotation` — rotation about the root's own origin, never about
/// the scene's).
fn apply_yaw(
    commands: &mut Commands,
    root: Entity,
    spot: Vec3,
    stage: Quat,
    stage_scale: f32,
    yaw: f32,
) {
    commands.entity(root).insert(
        Transform::from_translation(spot)
            .with_rotation(stage * Quat::from_rotation_y(yaw))
            .with_scale(Vec3::splat(stage_scale)),
    );
}

/// The stage's **authored frame**: attachment `bone`'s chain, composed from the constant rotation
/// and scale the scene author parked on it.
///
/// A stage is an M2 *attachment*, and an attachment carries a frame, not just a point. The
/// reference composes the whole thing: `child+0xfc = child+0xbc × (T(att.pos) · parentBone[att.bone])`
/// (`MatrixMultiply` at `0x71439b`, inside `0x714260`) — **rotation and scale go in, not only the
/// translation**.
/// Five of the six shipped `UI_*` scenes leave that bone unrotated,
/// because their camera is authored looking straight down the stage's +X and a WoW model's default
/// facing already looks that way (measured on the files: Orc +0.02°, Dwarf +0.62°, NightElf
/// −0.24°, Scourge 0.00°, Tauren +0.02° between the camera's eye→target and +X).
///
/// **`UI_Human`'s is the exception, and taking only the attachment *point* was why every Human
/// stood angled away on the login and select screens (1533).** That scene is a lifted chunk of
/// Elwynn at world (−225, −81), and its camera sits at azimuth −18.2° from the stage rather than
/// on the axis; the author aimed the character at it by keying the stage bone (bone 4) with a
/// constant **−16.5°** yaw — two identical quaternion keys `(0, 0, −0.14349, 0.98965)` on global
/// sequence 1, byte-read from the shipped `UI_Human.m2`.
///
/// Read off the model's baked global-sequence channels because that is where a *constant* M2
/// rotation lives: vanilla's rest pose is identity rotations with the pivot carrying bind position,
/// so an author who wants a bone held at an angle keys it and parks the key. The channels are
/// already conjugated into Bevy space by the loader, so the composed quaternion drops straight into
/// the character root's transform.
///
/// The **scale** rides the same channels and is uniform on every shipped scene — UI_Human parks
/// 0.97 on both its attachment bones, UI_Tauren 1.04 on the character's and 0.7324 on the pet's,
/// the other four none — so it is carried as one factor rather than a `Vec3`, and `x` is read
/// because a non-uniform stage would be an authoring surprise, not a case to silently average.
fn stage_frame(model: &M2Model, bone: u16) -> (Quat, f32) {
    let Some(anims) = model.animations.as_ref() else {
        return (Quat::IDENTITY, 1.0);
    };
    let mut rot = Quat::IDENTITY;
    let mut scale = 1.0;
    let mut idx = bone;
    // Cycle-guarded by the joint count, exactly as the pivot walk beside it is.
    for _ in 0..=model.skeleton.joints.len() {
        if let Some(g) = anims.global_bones.iter().find(|g| g.bone == idx) {
            // Parent-first composition up the chain; `t = 0` because these are parked keys — a
            // stage bone that genuinely animated would need the live joint, and none does.
            if let Some(c) = g.rotation.as_ref() {
                rot = c.sample(0.0) * rot;
            }
            if let Some(c) = g.scale.as_ref() {
                scale *= c.sample(0.0).x;
            }
        }
        let Some(joint) = model.skeleton.joints.get(usize::from(idx)) else {
            break;
        };
        match u16::try_from(joint.parent) {
            Ok(parent) => idx = parent,
            Err(_) => break, // -1 = root reached
        }
    }
    (rot, scale)
}

/// The phase-3 preview instrument (`WOW_CREATE_TEST="race,sex[,class,skin,face,hairStyle,hairColor,facialHair]"`,
/// decisions 0423 + 0527): set the create-preview look once and show the booth's bake full-screen (an
/// `ImageNode` over everything), so the create body is verifiable server-less before the screen exists
/// (phase 4). Inert without the env. `class` (default 1 = Warrior) selects the CharStartOutfit the
/// preview dresses in; every appearance dial defaults to 0.
pub(super) fn drive_create_test(
    mut commands: Commands,
    mut preview: ResMut<GluePreview>,
    portraits: Res<PortraitImages>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    *done = true;
    let Ok(spec) = std::env::var("WOW_CREATE_TEST") else {
        return;
    };
    let mut it = spec.split(',').map(|s| s.trim().parse::<u8>().unwrap_or(0));
    let mut next = |d: u8| it.next().unwrap_or(d);
    let look = CreateLook {
        race: next(1),
        sex: next(0),
        class: next(1),
        skin: next(0),
        face: next(0),
        hair_style: next(0),
        hair_color: next(0),
        facial_hair: next(0),
    };
    info!(
        "create-test: race {} sex {} class {} — skin {} face {} hair {} hairColor {} facial {}",
        look.race,
        look.sex,
        look.class,
        look.skin,
        look.face,
        look.hair_style,
        look.hair_color,
        look.facial_hair
    );
    preview.scene = Some(GlueScene::Race(look.race));
    preview.look = Some(GlueLook::Create(look));
    // Show the booth's RTT full-screen (behind nothing) so a live shot / capture eyeballs the body.
    if let Some(PortraitSource::Live(image)) = portraits.0.get(GLUE_SLOT) {
        commands.spawn((
            ImageNode::new(image.clone()),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            GlobalZIndex(2000),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::ModelLight;
    use benilla_formats::M2Light;

    /// **Only a select look stands a pet, and only when the wire named one.**
    ///
    /// `petDisplayId == 0` is the entire client-side gate: the server suppresses the triple for
    /// everything but a living hunter or warlock (vmangos `Player::BuildEnumData`), so a class or
    /// ghost test here would be a second, drifting copy of a rule we do not own. The create screen
    /// builds a character that has no pet yet and must never show one.
    #[test]
    fn a_pet_stands_only_for_a_select_look_whose_wire_named_one() {
        let mut c = benilla_protocol::Character {
            guid: 1,
            name: "Hunter".into(),
            race: 1,
            class: 3,
            gender: 0,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
            level: 60,
            zone: 0,
            map: 0,
            position: benilla_protocol::wire::Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            flags: 0,
            equipment: [benilla_protocol::CharEnumItem::default(); 19],
            pet_display_id: 517,
            pet_level: 60,
            pet_family: 1,
        };
        assert_eq!(
            GlueLook::Select(SelectLook::from(&c)).pet(),
            Some(PetLook {
                display_id: 517,
                level: 60,
                family: 1,
            }),
            "the roster row's whole pet triple rides the select look — the level and family are \
             its size, not decoration"
        );

        c.pet_display_id = 0;
        assert_eq!(
            GlueLook::Select(SelectLook::from(&c)).pet(),
            None,
            "a zeroed triple is 'no pet' — the only gate the client applies"
        );

        // The create screen: a character being built has no pet, whatever class is picked.
        let create = GlueLook::Create(CreateLook {
            race: 1,
            sex: 0,
            class: 3, // hunter
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
        });
        assert_eq!(create.pet(), None);
    }

    /// A same-size `resize_target` must not touch the image at all. `Assets::get_mut` queues
    /// `AssetEvent::Modified` whether or not the caller writes, and this target is the glue
    /// scene's fullscreen render target — one needless borrow per frame re-uploaded 22 MB per
    /// frame at 3200×1800 while the login screen sat still.
    #[test]
    fn a_same_size_resize_does_not_mark_the_target_modified() {
        use bevy::asset::AssetEvent;
        use bevy::image::Image;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>();
        let handle = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            images.add(Image::new_target_texture(
                64,
                64,
                bevy::render::render_resource::TextureFormat::bevy_default(),
                None,
            ))
        };
        // Drain the Added event from the insert above.
        app.update();
        let drain = |app: &mut App| {
            let world = app.world_mut();
            let mut events = world.resource_mut::<Messages<AssetEvent<Image>>>();
            let n = events.drain().count();
            n
        };
        drain(&mut app);

        // Same size, ten frames: silence.
        for _ in 0..10 {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            resize_target(&mut images, &handle, 64, 64);
        }
        app.update();
        assert_eq!(
            drain(&mut app),
            0,
            "a no-op resize must not queue AssetEvent::Modified — every one of those re-uploads \
             the whole target to the GPU"
        );

        // A real size change still lands.
        {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            resize_target(&mut images, &handle, 128, 96);
        }
        app.update();
        assert!(drain(&mut app) > 0, "a real resize must still be published");
        let images = app.world().resource::<Assets<Image>>();
        let image = images.get(&handle).expect("target still there");
        assert_eq!((image.width(), image.height()), (128, 96));
    }

    fn light(
        light_type: u16,
        pos: [f32; 3],
        bone_z: [f32; 3],
        dif: [f32; 3],
        di: f32,
        amb: [f32; 3],
        ai: f32,
    ) -> ModelLight {
        ModelLight {
            def: M2Light {
                light_type,
                bone: -1,
                position: pos,
                bone_z,
                ambient_color: amb,
                ambient_intensity: ai,
                diffuse_color: dif,
                diffuse_intensity: di,
                attenuation_start: 2.0,
                attenuation_end: 5.0,
                visibility_off: false,
            },
            bone_pivot: [0.0; 3],
        }
    }

    /// GOLDEN — the ghost select screen's hardcoded sun (see [`ghost_rig`] for the citations).
    ///
    /// The **sign** is the assertion that matters: a wrong sign is 180° off, a character lit from
    /// above versus from below. `CGLight+0x24` is a from-light vector, not a to-light one:
    /// `0x71bce0` negates all three components (`0x71be7c`/`0x71be81`/`0x71be86`) before it
    /// accumulates, and the fixed-function commit negates too, so both lanes agree on `(0,0,+1)`.
    /// The trap is why this is pinned rather than trusted to a comment: an AUTHORED M2 light
    /// already carries one negation (`0x718960` stores `-Bz`, which is why [`scene_rig`] feeds
    /// `+bone_z`), so reusing the authored convention here double-negates and lights the ghost from
    /// underneath.
    #[test]
    fn the_ghost_sun_is_one_overhead_directional_over_a_wiped_gather() {
        let rig = ghost_rig();
        // The gather is WIPED — `0x71bc30`'s `mov ecx,0x6d; rep stosd`. No stage lamps survive.
        assert!(
            rig.points.is_empty(),
            "a ghost selection discards the scene-DB gather whole"
        );
        // Exactly one directional.
        assert_eq!(rig.lobes.len(), 1);
        let (to_light, diffuse) = rig.lobes[0];
        // Bevy +Y is up: `wow_to_bevy([0,0,1])`. Overhead, not underfoot.
        assert_eq!(to_light, Vec3::Y, "lit from directly overhead (WoW Z up)");
        // `LightIntBand` 37 = 0x005E99C6 → (94,153,198); 38 = 0x001A3855 → (26,56,85).
        let byte = |c: f32| (c * 255.0).round() as u32;
        assert_eq!(
            diffuse.map(byte),
            [94, 153, 198],
            "LightParams row 3, sub 0"
        );
        assert_eq!(rig.ambient.map(byte), [26, 56, 85], "…and sub 1");
    }

    /// GOLDEN — the rig fold's classification law (the gather `0x718960`): directionals
    /// accumulate (ambient tracks summed; diffuse×intensity as per-light SH lobes whose to-light is
    /// the **bone's +Z axis**, never the def position), points land on the point table verbatim
    /// (colour × intensity, position wow→bevy) — never folded into the sun, and their dead authored
    /// attenuation radii never scale anything.
    #[test]
    fn rig_fold_classifies_directional_vs_point() {
        let lights = [
            // A warm directional key + a dedicated ambient-only directional (the UI_* pattern).
            // Positions are decoys — a directional's direction comes from bone_z alone.
            light(
                0,
                [99.0, 99.0, 99.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.5, 0.2],
                2.0,
                [1.0; 3],
                0.0,
            ),
            light(
                0,
                [99.0, 99.0, 99.0],
                [0.0, 1.0, 0.0],
                [1.0; 3],
                0.0,
                [0.4, 0.5, 0.6],
                0.5,
            ),
            // A stage-local point light (the NightElf/Scourge character light).
            light(
                1,
                [1.0, 2.0, 3.0],
                [0.0, 0.0, 1.0],
                [0.7, 0.8, 1.0],
                2.5,
                [1.0; 3],
                0.0,
            ),
        ];
        let rig = scene_rig(&lights);
        // Ambient = Σ ambient_color × ambient_intensity, across every light.
        assert_eq!(rig.ambient, [0.2, 0.25, 0.3]);
        // Exactly the one point light, position converted, colour × intensity — over-gamut
        // preserved. This fixture's `(0.7, 0.8, 1.0) × 2.5` is deliberately over-driven so it pins
        // the mechanism: the raw product `(1.75, 2.0, 2.5)` leaves the parse untouched, and
        // `LightBlob::point` commits it RAW from here (the reference's `0x71ca80` encode is undone
        // by the `0x593040` decode) — the saturation the eye sees happens at the receiving
        // vertex's lighting clamp, never on this path.
        assert_eq!(rig.points.len(), 1);
        assert_eq!(
            rig.points[0].0,
            benilla_assets::coords::wow_to_bevy([1.0, 2.0, 3.0])
        );
        let c = rig.points[0].1;
        assert!(
            (c[0] - 1.75).abs() < 1e-6 && (c[1] - 2.0).abs() < 1e-6 && (c[2] - 2.5).abs() < 1e-6,
            "over-gamut preserved: {c:?}"
        );
        // Each directional's bone_z (wow→bevy) becomes a to-light lobe; the zero-intensity
        // ambient-only light contributes a zero lobe (colour 0). Never the point light.
        assert_eq!(
            rig.lobes,
            vec![
                (
                    benilla_assets::coords::wow_to_bevy([1.0, 0.0, 0.0]),
                    [2.0, 1.0, 0.4],
                ),
                (
                    benilla_assets::coords::wow_to_bevy([0.0, 1.0, 0.0]),
                    [0.0, 0.0, 0.0],
                ),
            ]
        );
    }

    /// **The glue booth swaps as a PAIR** ([`PendingSwap`], [`may_swap`]). The defect it closes,
    /// measured on a cold walk of a seven-character roster: clicking an orc from a human stood the
    /// human's display in the orc stage for 53 ms, and clicking a human back from an orc — a body
    /// already cached, a stage still loading — stood the human in the *orc* stage for 34 ms. The
    /// reference cannot express either state: `SelectCharacter` loads the background and builds
    /// the character inside one blocking call.
    #[test]
    fn a_stage_swaps_only_beside_the_character_that_asked_for_it() {
        // Nothing standing (the login → select hop, an empty account): no pair to break.
        assert!(may_swap(false, false, false, 0.0));

        // A character IS standing: neither half alone may swap the stage.
        assert!(
            !may_swap(true, true, false, 0.0),
            "the stage's art landed first — swapping now stands the OUTGOING body in it"
        );
        assert!(
            !may_swap(true, false, true, 0.0),
            "the body assembled first — swapping now is the same mispairing, reversed"
        );
        assert!(
            may_swap(true, true, true, 0.0),
            "both halves ready: swap, in one frame"
        );

        // 0737's rule: the hold is bounded. A look whose assembly can never complete must not
        // freeze the screen on the previous character.
        assert!(!may_swap(true, true, false, GLUE_SWAP_HOLD_SECS - 0.01));
        assert!(may_swap(true, true, false, GLUE_SWAP_HOLD_SECS));
    }
}
