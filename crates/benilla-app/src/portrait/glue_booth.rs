//! The glue booth: one booth slot, with no wire entity, standing the glue screens' background
//! scene (`UI_MainMenu` at login, `UI_<Race>` at create and select) with a live, rotatable
//! character in it. The parts come pre-assembled in [`GluePreviewBake`]; this module lights,
//! poses, seats and yaws them, and the bake lands in [`super::PortraitImages`] under
//! [`GLUE_SLOT`].

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
// `GLUE_LAYER` lives with the other booth layers in [`super`], so no two booths share one.
/// The scene-less fallback target size; with a scene up the target follows the window.
const GLUE_SIZE: u32 = 1024;

/// The create screen's look: race, sex, class and the five appearance dials. The body wears the
/// (race, class, sex) `CharStartOutfit`, so a class change re-dresses it (reference: `SelectClass`
/// `0x470f50` → `0x470800`).
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

/// The select screen's look: the roster entry's appearance and equipment, off `SMSG_CHAR_ENUM`.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct SelectLook {
    pub(crate) race: u8,
    pub(crate) sex: u8,
    pub(crate) skin: u8,
    pub(crate) face: u8,
    pub(crate) hair_style: u8,
    pub(crate) hair_color: u8,
    pub(crate) facial_hair: u8,
    /// `CHARACTER_FLAG_*` bits; the builder honours hide-helm and hide-cloak.
    pub(crate) flags: u32,
    pub(crate) equipment: [benilla_protocol::CharEnumItem; 19],
    /// The roster row's pet triple; the level and family set its size through `CreatureFamily`'s
    /// ramp, so a freshly tamed wolf stands smaller than a level-60 one.
    pub(crate) pet_display_id: u32,
    pub(crate) pet_level: u32,
    pub(crate) pet_family: u32,
}

/// The select pet, as the booth needs it: which creature, and the two words that size it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PetLook {
    /// `CreatureDisplayInfo` id, never 0 ([`GlueLook::pet`] filters that out).
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

    /// The pet beside the character; only a select look carries one.
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

/// Which glue background scene shows: the login screen's main menu, or a race's `UI_<Race>` stage.
/// The main menu always fogs; a race stage fogs at create only.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlueScene {
    /// `UI_MainMenu`, the login screen's burning gate (`AccountLogin.xml`'s ModelFFX).
    MainMenu,
    /// A race's `UI_<Race>` stage (Gnome uses Dwarf's, Troll uses Orc's).
    Race(u8),
}

/// The glue screens' input to the booth: the scene (`None` tears the booth down; select shows Orc
/// before a list arrives), the standing look (`None` for an empty stage), and the yaw in radians,
/// which create resets to the reference's −15° and select to 0; a yaw change never re-bakes.
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

/// The glue background scene, the reference's fullscreen `ModelFFX`: `SetBackgroundModel` loads
/// `UI_<Race>.mdx`, plays sequence 0 and shows camera 0. Its root never yaws; the character
/// stands on its attachment 0, and its camera drives the booth camera while it shows.
#[derive(Resource)]
pub(crate) struct CreateScene {
    root: Entity,
    token: Option<&'static str>,
    handle: Option<Handle<M2Model>>,
    spawned: bool,
    /// The authored camera 0, captured at spawn.
    cam: Option<benilla_assets::PortraitCamera>,
    /// The shipped scene's art extent ([`benilla_formats::shipped_glue_art_extent`]): caps how far
    /// [`glue_scene_framing`] opens upward on a narrow window. `None` with no scene.
    art: Option<ArtExtent>,
    /// `Some(aspect)` while the window is wider than [`super::framing::GLUE_BOX_ASPECT`]: the
    /// camera renders into a centred viewport of this aspect ([`pillarbox_glue_scene`]). Set from
    /// the window, not the scene, so the chrome's canvas never moves on a race change.
    viewport_aspect: Option<f32>,
    /// The character's stage spot, scene attachment 0 in Bevy model space (`ZERO` with no scene);
    /// the body seats on attachment 0 at select too (`0x473039`).
    char_spot: Vec3,
    /// The stage's authored frame, attachment 0's bone rotation and uniform scale
    /// ([`stage_frame`]); identity and 1.0 with no scene.
    char_facing: Quat,
    char_scale: f32,
    /// The pet's spot, scene attachment 1 (`0x47306b` seats the secondary model there). Every
    /// `UI_*` scene authors exactly attachments 0 and 1, both on parentless root bones.
    pet_root: Entity,
    pet_spot: Vec3,
    /// Attachment 1's bone frame, read like [`Self::char_facing`]. UI_Human's holds the same
    /// −16.5° yaw and 0.97 scale as attachment 0; UI_Tauren's holds 0.7324 on the pet's bone alone.
    pet_facing: Quat,
    pet_stage_scale: f32,
    /// The revision of the pet bake standing at [`Self::pet_spot`], so a re-bake happens once.
    pet_baked: Option<u64>,
    /// Whether the spawn wrote fog enabled (create); the scene rebuilds when it flips.
    fog: bool,
    /// Whether the buffer holds the ghost rig ([`ghost_rig`]); a flip rewrites the buffer in
    /// place, like the reference's per-frame fill callback, so the stage's emitters keep running.
    ghost: bool,
    /// The scene's light buffer: its authored rig ([`SceneRig`]) plus the per-race fog
    /// (`CharModelFogInfo`) at create only; select renders unfogged (`0x472110`).
    light: Option<bevy::render::render_resource::Buffer>,
    /// Booth twins of the character's materials against the scene buffer (`BoothLight::variants`).
    variants: std::collections::HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
    /// Bumped on every scene spawn; `sync_glue_booth` keys on it to re-light the character.
    rev: u64,
    /// The requested stage not yet committed ([`PendingSwap`]).
    pending: Option<PendingSwap>,
}

/// A requested stage swap, held until both halves are ready. The scene M2 and the character's
/// assembly land frames apart, and whichever came first would stand the wrong character in the
/// wrong stage; the reference's `SelectCharacter` loads both in one blocking call. The loads start
/// on the click; only the commit waits.
struct PendingSwap {
    /// The requested scene token (`UI_<token>`).
    token: &'static str,
    /// Its model, loading since the click.
    handle: Handle<M2Model>,
    /// `Time<Real>` at the click, for [`GLUE_SWAP_HOLD_SECS`].
    since: f32,
}

/// The bound on that hold, so a look whose assembly never completes cannot freeze the screen.
const GLUE_SWAP_HOLD_SECS: f32 = 2.0;

/// The scene's model-name token, `Interface\Glues\Models\UI_<token>\UI_<token>.mdx`. The race
/// mapping is `GlueParent.lua`'s `SetBackgroundModel`; the main menu is `AccountLogin.xml`'s.
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

/// Whether the stage may swap this frame: nothing coherent is `standing`, both the requested art
/// and body are ready, or the hold has reached [`GLUE_SWAP_HOLD_SECS`].
fn may_swap(standing: bool, art_ready: bool, body_ready: bool, waited: f32) -> bool {
    !standing || (art_ready && body_ready) || waited >= GLUE_SWAP_HOLD_SECS
}

/// A glue scene's authored M2 light rig, folded as the reference's gather (`0x718960`):
/// directional ambients sum, each directional's diffuse × intensity becomes an SH lobe
/// ([`benilla_world::lighting::prop_probe_coeffs`]), and point lights go to the point table with
/// the fixed falloff `1/(0.7·d + 0.03·d²)`; the reference ignores the authored attenuation radii.
struct SceneRig {
    /// The summed directional ambient (every UI_* point light authors ambient ×0).
    ambient: [f32; 3],
    /// The directional lobes, toward-light and committed; folded into probe slot 0.
    lobes: Vec<(Vec3, [f32; 3])>,
    /// The point lights: Bevy position and committed colour.
    points: Vec<(Vec3, [f32; 3])>,
}

/// Range packed with a scene point light: the reference gather has no range gate (≤3 nearest).
const SCENE_POINT_RANGE: f32 = 1.0e6;

/// Fold the scene model's lights into a [`SceneRig`]. A directional's to-light direction is its
/// bone's local +Z, row 2 of the light bone's pose matrix (`0x718a76`), never the def position.
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
            // `LightBlob::point` commits the colour raw, as the world table does: the reference's
            // `0x71ca80` encode is undone by the `0x593040` decode.
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

/// The ghost select screen's rig: the reference's fill callback `0x472150`, on the scene, the
/// character and the pet, runs only for a record with `CHARSELECT+0xfc & 0x2000`. It wipes the
/// gather (`0x71bc30`: no authored ambient, no point lights) and injects one directional.
///
/// `0x472280` writes `(0,0,-1)` as a from-light vector and `0x71bce0` negates it, so the light is
/// overhead; an authored light is negated twice (`0x718960` stores `-Bz`), this one once.
///
/// The colours are `LightParams` row 3, the death preset (`0x4721e2`), through `0x6d6ad0` to
/// `LightIntBand` 37 (diffuse) and 38 (ambient) at time 0. Both bands are single-key in 1.12.1, and
/// the glue screens have no `LightParams` state to read, so they are literals.
fn ghost_rig() -> SceneRig {
    SceneRig {
        // `LightIntBand` 38 = 0x001A3855 → (26, 56, 85).
        ambient: [26.0 / 255.0, 56.0 / 255.0, 85.0 / 255.0],
        // `LightIntBand` 37 = 0x005E99C6 → (94, 153, 198), on the to-light `(0,0,+1)`.
        lobes: vec![(
            benilla_assets::coords::wow_to_bevy([0.0, 0.0, 1.0]),
            [94.0 / 255.0, 153.0 / 255.0, 198.0 / 255.0],
        )],
        // Wiped: the stage's own lamps do not survive.
        points: Vec::new(),
    }
}

/// Pack a [`SceneRig`] into the glue scene's light buffer. The core rows carry the plain ambient
/// with a zero sun for any non-rig lane; the directional fold lands in probe slot 0 (`MeshTag` 0).
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

/// The per-scene fog, colour and far (near is 0): the race rows are `GlueParent.lua`'s
/// `CharModelFogInfo`; the main menu's is `AccountLogin.xml`'s `<FogColor>` and `fogFar`.
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

/// One assembled preview mesh, the tuple-driven counterpart of [`super::PortraitPart`].
#[derive(Clone)]
pub(crate) struct PreviewPart {
    pub(crate) static_mesh: Handle<Mesh>,
    pub(crate) skinned_mesh: Option<Handle<Mesh>>,
    pub(crate) material: Handle<WowModelMaterial>,
    /// The batch's animated material alpha: `None` from the character assembly, set for a pet.
    pub(crate) alpha_anim: Option<std::sync::Arc<benilla_formats::AlphaAnim>>,
    /// Translucency twins ([`BoothTwins`]), drawn when the bake's instance alpha is below 1.
    pub(crate) twins: BoothTwins,
}

/// One equipment rider (helm, shoulder, sheathed weapon), seated under the body's `bone` joint at
/// `offset`; the booth twin of [`crate::portrait::PortraitRider`].
#[derive(Clone)]
pub(crate) struct PreviewRider {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) material: Handle<WowModelMaterial>,
    pub(crate) bone: u16,
    pub(crate) offset: Vec3,
    /// As [`PreviewPart::twins`]; an attached model composes onto its wearer's instance alpha.
    pub(crate) twins: BoothTwins,
}

/// One camera-facing batch on the preview, the world's billboard card rebuilt for the booth
/// camera ([`super::booth::face_booth_billboards`]): the character's eye-glow (geoset 302, on its
/// rigged bone, offset `ZERO`), an equipped item's batch (a wand's gem; 270 of the 2681 `Item\`
/// models author one), or an item glow's (`Sparkle_A.m2`, `ItemVisuals` 28). An item batch's
/// offset carries the attach point and the batch's own pivot.
#[derive(Clone)]
pub(crate) struct PreviewBillboard {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) material: Handle<WowModelMaterial>,
    pub(crate) bone: u16,
    /// The card's pivot in that bone's joint frame (Bevy axes).
    pub(crate) offset: Vec3,
    pub(crate) kind: benilla_formats::BillboardKind,
    /// As [`PreviewPart::twins`].
    pub(crate) twins: BoothTwins,
}

/// One effect-bearing model on the preview: its emitters and its seat, a body bone and an offset
/// in that bone's frame, hosted as [`sync_glue_scene`] hosts a scene's braziers. Either an
/// equipped item's own emitters (a pauldron's sparkle, a torch's flame) at the slot's attach
/// point, or a weapon's `ItemVisuals` glow (32 of the 35 shipped glow models are pure emitters)
/// at the attach point plus the glow slot's offset on the weapon.
#[derive(Clone)]
pub(crate) struct PreviewEffects {
    pub(crate) bone: u16,
    pub(crate) offset: Vec3,
    pub(crate) emitters: Vec<benilla_assets::ModelEmitter>,
}

/// The select screen's ghost treatment of the body, applied by the reference at `0x4727f0` for a
/// record with `CHARSELECT+0xfc & 0x2000`: `SpellVisualKit` 989 (`0x47280f`), the state kit of
/// spell 8326 "Ghost" that the world ghost rides, resolved through
/// [`crate::aura_visual::node_for`]. There is no ramp: the world's 1000 ms alpha ease
/// (`0x614f80`) is never reached, and the values go straight through `0x710cb0`/`0x710cf0`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct GhostKit {
    /// The whole-model modulate colour (CharProc 1, `cc+0x184..0x18c`).
    pub(crate) tint: [u8; 3],
    /// The whole-model alpha (CharProc 14, `cc+0x180`), applied instantly.
    pub(crate) alpha: f32,
}

/// The entities-side builder's output for the current look, with a revision that bumps on a new
/// bake or a clear; the booth re-bakes only when `revision` changes.
#[derive(Resource, Default)]
pub(crate) struct GluePreviewBake {
    pub(crate) look: Option<GlueLook>,
    pub(crate) display_id: u32,
    pub(crate) parts: Vec<PreviewPart>,
    pub(crate) riders: Vec<PreviewRider>,
    /// Every emitter-bearing model the look wears ([`PreviewEffects`]).
    pub(crate) effects: Vec<PreviewEffects>,
    /// The look's camera-facing batches ([`PreviewBillboard`]), faced to the booth camera.
    pub(crate) billboards: Vec<PreviewBillboard>,
    /// Per-hand weapon grip `[right, left]`, closing into `HandsClosed` (`CloseHand` `0x479660`).
    pub(crate) grip: [bool; 2],
    /// The selection's ghost kit ([`GhostKit`]); its effect models are already in the lists above.
    pub(crate) ghost: Option<GhostKit>,
    pub(crate) revision: u64,
}

/// The select screen's pet, the reference's secondary model (`record+0x114`). Its own resource so
/// a pet landing late does not re-bake the character. Empty unless the character is a living
/// hunter or warlock (the server zeroes the triple otherwise).
#[derive(Resource, Default)]
pub(crate) struct GluePetBake {
    /// The pet's `CreatureDisplayInfo` id; 0 means no pet.
    pub(crate) display_id: u32,
    pub(crate) parts: Vec<PreviewPart>,
    /// The pet's camera-facing batches (a voidwalker's eyes).
    pub(crate) billboards: Vec<PreviewBillboard>,
    /// The pet model's own emitters (the imp's flame jets), riding its own joints.
    pub(crate) emitters: Vec<benilla_assets::ModelEmitter>,
    /// The render scale, the `CreatureFamily` level ramp
    /// ([`benilla_formats::CreatureFamily::scale_at`]): the reference overwrites the display's
    /// scale product with it (`0x472e87`). The glue pet has no wire object to carry a scale.
    pub(crate) scale: f32,
    pub(crate) revision: u64,
}

/// Stand up the glue booth (called from [`super::setup_booths`]).
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
        // The shared booth view shape; [`super`] says why every booth camera spawns through it.
        super::booth_view_shape(),
        Camera {
            order: -100 + GLUE_LAYER as isize,
            // Transparent: the glue screens composite the scene straight over the page, and
            // FfxGlow's combine carries the scene alpha through.
            clear_color: ClearColorConfig::Custom(Color::NONE),
            // The output clear outside a pillarboxed viewport: black, not Bevy's default global
            // `ClearColor`, which the world sets to the fog colour.
            output_mode: bevy::camera::CameraOutputMode::Write {
                blend_state: None,
                clear_color: ClearColorConfig::Custom(Color::BLACK),
            },
            ..default()
        },
        bevy::camera::RenderTarget::Image(image.clone().into()),
        // The reference's glue FFX pass pair around the scene (`0x46fad3`/`0x46fae0`); the
        // GlueXML frames draw over it, so a ghost selection tints the scene and not the UI.
        benilla_world::ffx_glow::FfxGlow::GLUE_SCENE,
        // Placeholder: `sync_glue_booth` sets transform and projection on the first bake.
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
            // `gate_booth_cameras` keeps the glue camera live while a scene shows.
            wake: 0,
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
    // The background scene's own root; the character root yaws, this one never does.
    let scene_root = commands
        .spawn((Transform::IDENTITY, Visibility::Visible, layer.clone()))
        .id();
    // The pet's root never yaws: the reference's facing call turns only the character (`0x4730e0`).
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

/// Drive the glue FFX state ([`benilla_world::ffx_glow::GlueFfx`]) off the live look: the pass
/// installed and the pinned `LightParams.glow`.
///
/// | look | reference | pass | glow |
/// |---|---|---|---|
/// | none (login, empty account) | `CSimpleModelFFX::OnShow 0x46fa60` | glow | `*0x8380b4` 0.30 |
/// | a create look | same, no select build runs | glow | 0.30 |
/// | a living select record | `0x472ff7`/`0x473002` | glow | `*0x838574` 0.40 |
/// | a ghost select record | `0x472fde`/`0x472fe9` | death | `*0x838570` 0.15 |
///
/// An empty account is the OnShow state: `0x472950` is gated `-1 ≤ [0x83856c] < [0xb42140]`.
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

/// Drive the background scene ([`CreateScene`]): load and spawn the race's `UI_*` model, size the
/// target to the window, seat the character, and hold the booth camera on the scene's camera 0.
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
    // One tuple for the 16-SystemParam ceiling.
    particle_assets: (
        ResMut<benilla_world::rig_palette::RigPalettes>,
        ResMut<benilla_world::rig_palette::RigPaletteMirrors>,
        ResMut<benilla_world::model_forms::ModelForms>,
        ResMut<Assets<Mesh>>,
        ResMut<benilla_world::instance_tint::InstanceTintMirrors>,
        ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
        ResMut<benilla_world::doodad_anim::TintAnimMaterials>,
        ResMut<benilla_world::mat_anim_table::MatAnimTable>,
        ResMut<benilla_world::mat_anim_table::MatAnimMirrors>,
    ),
    // The swap gate's inputs ([`PendingSwap`]).
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
            // Strip the rig state (pose buffer, player, palette slot): the root outlives its bakes.
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

    // The frame box is set from the window alone, before a token resolves, so the chrome's canvas
    // ([`crate::glue::GlueCanvas`]) neither moves with the race nor flickers during a swap.
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
    // A race stage fogs at create and not at select (`0x472110` replaces the fog callback); the
    // main menu always fogs.
    let fog = match which {
        GlueScene::MainMenu => true,
        GlueScene::Race(_) => matches!(preview.look, Some(GlueLook::Create(_))),
    };
    // The ghost light fork (`0x472150`, [`ghost_rig`]), shared by the scene, character and pet.
    let ghost = matches!(
        preview.look,
        Some(GlueLook::Select(l)) if l.flags & benilla_protocol::CHARACTER_FLAG_GHOST != 0
    );
    if scene.token == Some(token) {
        // The requested stage is already standing: no swap pends.
        scene.pending = None;
    }
    if scene.token != Some(token) {
        // The pair swaps together or not at all ([`PendingSwap`]).
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
        // `build_glue_preview` publishes the look it built.
        let art_ready = m2s.get(&pending.handle).is_some();
        let body_ready = bake.look == preview.look;
        // An empty stage has no pair to break.
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
        // `viewport_aspect` stays: the frame is the window's, not the stage's.
        scene.char_spot = Vec3::ZERO;
        clear_pet(&mut commands, &mut scene);
        commands.entity(scene.root).despawn_related::<Children>();
        // The model may still be loading: strip the old rig state off the root now.
        super::clear_booth_rig(&mut commands, scene.root);
    } else if scene.spawned && scene.fog != fog {
        // Same scene, other screen: rebuild in place (the handle is cached, so it respawns now).
        scene.spawned = false;
        commands.entity(scene.root).despawn_related::<Children>();
    } else if scene.spawned && scene.ghost != ghost {
        // The selection crossed the ghost bit: re-light in place, as the reference's fill
        // callback does (`0x472150`); a respawn would restart the stage's emitters.
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
            // The character and the pet re-bake onto the rewritten buffer.
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
            return; // headless: no booth pipeline, no scene
        }
        // The scene's light, in its own buffer so the portraits' studio light is untouched.
        let stage = attachment_point(&model.skeleton, &model.attachments, 0).unwrap_or(Vec3::ZERO);
        // The stage's frame, not just its point ([`stage_frame`]).
        let (stage_facing, stage_scale) = model
            .attachments
            .iter()
            .find(|a| a.id == 0)
            .map_or((Quat::IDENTITY, 1.0), |a| stage_frame(model, a.bone));
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
        // The scene's rigs skin from this buffer's palette region.
        mirrors.0.insert("glue_scene", light.clone());
        // The per-instance tint region: the glue character's modulate colour is read from here.
        tint_mirrors.0.insert("glue_scene", light.clone());
        // The mat-anim delta table: the scene's materials sample `matanim[slot]` from this buffer.
        // The portrait booths stay off it ([`MatAnimMirrors`]): a bake photographs one instant.
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
        // Built now: one backdrop model, the booth-lane exception to the paced rule.
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
                // The authored batch order (index + 1) is all that separates batches sharing the
                // scene root's sort distance.
                let order = u16::try_from(pi + 1).unwrap_or(u16::MAX);
                let material = mats.off_world(s, s.texture.clone(), order, &light, true);
                // Texture transforms and M2Color tints run live, as in the reference's
                // per-frame animate kernel (`0x715f25`-`0x7163bc`). Every glue model's loops are
                // shared, none per-placement, so `host: None` is exact.
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
                    // The scene's authored per-batch alpha (UI_Tauren's 0.55 corner vignette).
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
            BoothMotion::Loop, // sequence 0 loops: flags wave, clouds drift
            [false, false],    // the backdrop scene has no hands to grip
            &[],               // …nor a character's eye-glow
            BoothInstance::default(),
        );
        // The scene's own emitters, fogged by the scene's buffer: the ModelFFX fog covers them.
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
        scene.art = benilla_formats::shipped_glue_art_extent(token);
        if let (Some(art), Some(cam)) = (scene.art, model.camera0.as_ref()) {
            let t0 = benilla_formats::authored_half_height(cam.fov);
            // Where the frame's edge lands on this stage's art.
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
        // The pet's seat: attachment 1, frame and all.
        scene.pet_spot =
            attachment_point(&model.skeleton, &model.attachments, 1).unwrap_or(Vec3::ZERO);
        let (pet_facing, pet_stage_scale) = model
            .attachments
            .iter()
            .find(|a| a.id == 1)
            .map_or((Quat::IDENTITY, 1.0), |a| stage_frame(model, a.bone));
        scene.pet_facing = pet_facing;
        scene.pet_stage_scale = pet_stage_scale;
        // A rebuilt scene moved the seat, so the standing pet re-seats.
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
        // Seat the character at the current facing (the yaw path below runs only on a change).
        apply_yaw(
            &mut commands,
            booth.root,
            scene.char_spot,
            scene.char_facing,
            scene.char_scale,
            preview.yaw,
        );
    }
    // The authored camera owns the booth camera while the scene shows, every frame.
    if let Some(cam) = scene.cam {
        let aspect = window
            .single()
            .map(|w| w.width() / w.height().max(1.0))
            .unwrap_or(4.0 / 3.0);
        let fwd = (cam.target - cam.eye).normalize_or_zero();
        let up = Quat::from_axis_angle(fwd, cam.roll) * Vec3::Y;
        // The record fov is diagonal; [`glue_scene_framing`] holds the authored 4:3 view box
        // where the reference converts at the display's aspect (the Deviation is stated there).
        // Far is kept generous (27.8 authored on Orc).
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

/// Pillarbox the glue booth camera to [`CreateScene::viewport_aspect`], black either side;
/// writes only on change.
pub(super) fn pillarbox_glue_scene(
    scene: Res<CreateScene>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut cams: Query<(&BoothCam, &mut Camera)>,
) {
    let Ok(w) = window.single() else {
        return;
    };
    let full_h = w.physical_height().max(1);
    // The same arithmetic the chrome's canvas insets by ([`glue_box_physical`]).
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

/// Resize the booth's render target if it is not already `w`×`h`.
fn resize_target(images: &mut Assets<Image>, target: &Handle<Image>, w: u32, h: u32) {
    // Gated: `Assets::get_mut` always marks the image modified, re-uploading the whole target.
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

/// Re-bake the glue booth when the parts or the scene's rig change, relit onto the scene's rig
/// (the reference's glue character is scene-lit) or the studio buffer, and spin it to the yaw.
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
    // The ghost's whole-model tint, the reference's `cc+0x184..0x18c`.
    mut tints: ResMut<benilla_world::instance_tint::InstanceTints>,
    mut last: Local<Option<(u64, f32, u64)>>,
) {
    let Some(booth) = booths.0.get_mut(GLUE_SLOT) else {
        return;
    };
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
    // `sync_glue_scene` runs first and commits a stage only beside its character, so a standing
    // stage matching the request means this bake may show ([`PendingSwap`]).
    let staged = scene
        .as_deref()
        .is_none_or(|s| s.token == preview.scene.map(scene_token));
    let rebake = staged && (last_rev != bake.revision || last_scene != scene_rev);
    let reyaw = last_yaw != preview.yaw;
    if !rebake && !reyaw {
        return;
    }

    if rebake {
        // Empty parts (nothing selected, or the model failed): clear the booth.
        if bake.parts.is_empty() {
            commands.entity(booth.root).despawn_related::<Children>();
            // The rig state on the root needs its own strip.
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
        // The rig comes from the display cache the builder gated on; if not ready, retry.
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
                // No parts-key retry here, so fall back rather than leave the pane empty.
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
        // One instance alpha per CM2, which attached models compose onto (`0x714000`).
        let instance = BoothInstance {
            alpha: bake.ghost.map_or(1.0, |g| g.alpha),
        };
        // Each twin relights onto its steady sibling's buffer.
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
                // `None`: the character assembly does not carry its alpha loops.
                alpha_anim: None,
                mat_anim: false,
            });
        }
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
        // `WOW_BOOTH_LOG=1`: which display was baked, and whether the look or the scene rig
        // triggered it.
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
            bake.grip,         // close each hand that holds a weapon
            &booth_billboards,
            instance,
        );
        // The whole-model tint ([`GhostKit::tint`], the reference's `0x472939` → `0x710cf0`),
        // per rig slot, identity when living so a ghost's colour never carries over.
        tints.set(
            booth_rig.slot(),
            match bake.ghost {
                Some(g) => benilla_world::instance_tint::pack(g.tint),
                None => benilla_world::instance_tint::IDENTITY,
            },
        );
        // The worn items' effects, through the reference's world attach primitive (`0x472c91` →
        // `0x47a0c0` → `0x4798c0`); the enum carries no enchant, so only intrinsic effects show.
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
        // The fallback framing with no scene up: the model's `<PlayerModel>` pane camera, at the
        // bake's aspect, as it shows only for the frames before the scene's camera takes over.
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
    // A yaw-only pass must not record revisions it did not commit, or a held pair never retries.
    *last = Some((
        if rebake { bake.revision } else { last_rev },
        preview.yaw,
        if rebake { scene_rev } else { last_scene },
    ));
}

/// Stand the select screen's pet on scene attachment 1 (the reference's secondary model,
/// `0x47306b`), relit onto the scene's rig, only while a scene is up. It does not yaw: the
/// reference's facing call (`0x4730e0`) turns the character alone.
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
    // No pet on this character: clear once and stop.
    if pet.display_id == 0 || pet.parts.is_empty() {
        if scene.pet_baked.is_some() {
            clear_pet(&mut commands, scene);
        }
        return;
    }
    if !scene.spawned {
        return; // no stage yet: nothing to stand on, nothing to be lit by
    }
    if scene.pet_baked == Some(pet.revision) {
        return; // already standing, unchanged
    }
    // The rig comes from the display cache that produced the parts; if not ready, retry.
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
        BoothMotion::Loop, // Stand, looping, like the character
        [false, false],    // …and no hands to grip with
        &booth_billboards,
        BoothInstance::default(),
    );
    // The pet's own emitters, on its own bones, scaled with `pet_root` (`sizeByInstanceScale`).
    let (emitters, fx_frames) = super::spawn_booth_own_emitters(
        &mut commands,
        &mut pet_rig,
        scene.pet_root,
        &booth.layer,
        Some(&light),
        &pet.emitters,
    );
    // `anchor` writes the pose buffer, `finish` commits it.
    pet_rig.finish(&mut commands);
    // The seat: attachment 1's whole frame, times the family ramp scale.
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
    // The rig state lives on the root, which the despawn does not reach.
    super::clear_booth_rig(commands, scene.pet_root);
    scene.pet_baked = None;
}

/// Seat the model root on the stage `spot` in the stage's frame, facing `yaw` (the reference's
/// `Model:SetRotation`, about the root's own origin).
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

/// The stage's authored frame: the constant rotation and uniform scale parked on attachment
/// `bone`'s chain. The reference composes an attachment's whole frame, not only its point
/// (`0x71439b` in `0x714260`). `UI_Human` keys its stage bones with a constant −16.5° yaw to face
/// its off-axis camera; the other race scenes leave them unrotated. The scale is uniform on every
/// shipped scene, so `x` stands for it.
fn stage_frame(model: &M2Model, bone: u16) -> (Quat, f32) {
    let Some(anims) = model.animations.as_ref() else {
        return (Quat::IDENTITY, 1.0);
    };
    let mut rot = Quat::IDENTITY;
    let mut scale = 1.0;
    let mut idx = bone;
    // Cycle-guarded by the joint count, as the pivot walk is.
    for _ in 0..=model.skeleton.joints.len() {
        if let Some(g) = anims.global_bones.iter().find(|g| g.bone == idx) {
            // `t = 0`: these are parked keys, and no stage bone animates.
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

/// The create preview instrument:
/// `WOW_CREATE_TEST="race,sex[,class,skin,face,hairStyle,hairColor,facialHair]"` sets the look
/// once and shows the bake full-screen. `class` defaults to 1 (Warrior), every dial to 0.
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
    // Show the booth's render target full-screen, so a live shot shows the body.
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

    /// `petDisplayId == 0` is the client's whole gate: the server zeroes the triple for all but a
    /// living hunter or warlock (vmangos `Player::BuildEnumData`).
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

    /// The ghost sun lights from overhead: `CGLight+0x24` is from-light and `0x71bce0` negates it.
    #[test]
    fn the_ghost_sun_is_one_overhead_directional_over_a_wiped_gather() {
        let rig = ghost_rig();
        // The gather is wiped (`0x71bc30`): no stage lamps survive.
        assert!(
            rig.points.is_empty(),
            "a ghost selection discards the scene-DB gather whole"
        );
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

    /// The rig fold (`0x718960`): directionals sum ambient and become lobes along bone +Z (never
    /// the def position); points land on the point table as colour × intensity.
    #[test]
    fn rig_fold_classifies_directional_vs_point() {
        let lights = [
            // A warm directional key and an ambient-only directional (the UI_* pattern);
            // positions are decoys.
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
        // Colour × intensity, deliberately over-gamut: `LightBlob::point` commits it raw.
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

    /// The reference loads the background and the character in one call (`SelectCharacter`).
    #[test]
    fn a_stage_swaps_only_beside_the_character_that_asked_for_it() {
        // Nothing standing (the login → select hop, an empty account): no pair to break.
        assert!(may_swap(false, false, false, 0.0));

        // A character is standing: neither half alone may swap the stage.
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

        // The hold is bounded: a look whose assembly never completes must not freeze the screen.
        assert!(!may_swap(true, true, false, GLUE_SWAP_HOLD_SECS - 0.01));
        assert!(may_swap(true, true, false, GLUE_SWAP_HOLD_SECS));
    }
}
