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

use super::booth::{redress_booth_batches, set_grip, BoothBatch, BoothRig};
use super::framing::{attachment_point, diag_to_vert, stock_pane_view, PORTRAIT_ASPECT};
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

/// Which glue background scene shows, by its model's token, `UI_<token>`: the login screen's main
/// menu (`AccountLogin.xml`'s `ModelFFX`) or a race's stage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GlueScene {
    MainMenu,
    Human,
    Orc,
    Dwarf,
    NightElf,
    Scourge,
    Tauren,
}

impl GlueScene {
    const ALL: [GlueScene; 7] = [
        GlueScene::MainMenu,
        GlueScene::Human,
        GlueScene::Orc,
        GlueScene::Dwarf,
        GlueScene::NightElf,
        GlueScene::Scourge,
        GlueScene::Tauren,
    ];

    /// The model-name token, `Interface\Glues\Models\UI_<token>\UI_<token>.mdx`.
    pub(crate) fn token(self) -> &'static str {
        match self {
            GlueScene::MainMenu => "MainMenu",
            GlueScene::Human => "Human",
            GlueScene::Orc => "Orc",
            GlueScene::Dwarf => "Dwarf",
            GlueScene::NightElf => "NightElf",
            GlueScene::Scourge => "Scourge",
            GlueScene::Tauren => "Tauren",
        }
    }

    /// A race's stage, `GlueParent.lua`'s `SetBackgroundModel` remap: Gnome stands in Dwarf's,
    /// Troll in Orc's.
    pub(crate) fn of_race(race: u8) -> Self {
        match race {
            2 | 8 => GlueScene::Orc,
            3 | 7 => GlueScene::Dwarf,
            4 => GlueScene::NightElf,
            5 => GlueScene::Scourge,
            6 => GlueScene::Tauren,
            _ => GlueScene::Human,
        }
    }

    /// The scene a model path names, by its file stem (`UI_<token>`, any case, any extension);
    /// `None` for a file that is none of the shipped scenes.
    pub(crate) fn from_model_path(path: &str) -> Option<Self> {
        let file = path.rsplit(['\\', '/']).next()?;
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        let token = stem.get(..3).filter(|p| p.eq_ignore_ascii_case("UI_"))?;
        let token = &stem[token.len()..];
        Self::ALL
            .into_iter()
            .find(|s| s.token().eq_ignore_ascii_case(token))
    }

    /// The fog the stock files give the scene, near 0: the race rows are `GlueParent.lua`'s
    /// `CharModelFogInfo`; the main menu's is `AccountLogin.xml`'s `<FogColor>` and `fogFar`.
    pub(crate) fn stock_fog(self) -> SceneFog {
        let (rgb, far) = match self {
            GlueScene::MainMenu => ([0.25, 0.06, 0.015], 1200.0),
            GlueScene::Orc => ([0.5, 0.5, 0.5], 270.0),
            GlueScene::Dwarf => ([0.85, 0.88, 1.0], 500.0),
            GlueScene::NightElf => ([0.25, 0.22, 0.55], 611.0),
            GlueScene::Scourge => ([0.0, 0.22, 0.22], 26.0),
            GlueScene::Tauren => ([1.0, 0.61, 0.42], 153.0),
            GlueScene::Human => ([0.8, 0.65, 0.73], 222.0),
        };
        SceneFog {
            rgb,
            near: 0.0,
            far,
        }
    }
}

/// A scene's fog: the colour and the span the fog fill callback (`0x76d680`) reads off the pane.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct SceneFog {
    pub(crate) rgb: [f32; 3],
    pub(crate) near: f32,
    pub(crate) far: f32,
}

/// The glue screens' input to the booth: the scene (`None` tears the booth down), the standing
/// look (`None` for an empty stage), the yaw in radians, which create resets to the reference's
/// −15° and select to 0 (a yaw change never re-bakes), and the scene's fog. The fog is the
/// pane's while the scene holds the fog fill callback `Model:SetModel` installs (`0x76d680`), and
/// none once `SetCharSelectBackground` put the select screen's in its place (`0x472150`).
#[derive(Resource)]
pub(crate) struct GluePreview {
    pub(crate) scene: Option<GlueScene>,
    pub(crate) look: Option<GlueLook>,
    pub(crate) yaw: f32,
    pub(crate) fog: Option<SceneFog>,
    /// The stock screens' pane the scene fills, `[left, right, top, bottom]` as window fractions
    /// from the top-left ([`super::framing::stock_pane_view`]); `None` frames at the window.
    pub(crate) pane: Option<[f32; 4]>,
    /// The stage's instance: a change starts the scene over, as each `SetModel` makes a new
    /// instance (`0x76cce0`), the same file included. The stock feeder numbers each (pane,
    /// instance) once, so a pane shown again on the instance it held brings its number back.
    pub(crate) stage: u64,
    /// The stages whose pane still holds them, shown or hidden: a scene left for another pane's
    /// is parked while its number is here, and resumes where it stopped when the number returns
    /// ([`CreateScene::parked`]). Empty from a feeder that numbers no panes.
    pub(crate) live: Vec<u64>,
    /// Bumped as a glue screen's scene shows: the `ModelFFX`'s OnShow pins the glow back to
    /// 0.30 (`0x46fa60`), which nothing else does ([`sync_glue_ffx`]).
    pub(crate) shown: u64,
    /// The scene's pane is hidden: the scene stays up, undrawn, and its clock stops, as a hidden
    /// `ModelFFX` keeps its model and scene and advances its clock only in its `OnUpdate`
    /// (`0x76d7f0`), which the visible-frame pump (`0x765650`) calls ([`pace_glue_clock`]).
    pub(crate) hidden: bool,
}

impl Default for GluePreview {
    fn default() -> Self {
        Self {
            scene: None,
            look: None,
            yaw: 0.0,
            fog: None,
            pane: None,
            stage: 0,
            shown: 0,
            hidden: false,
            live: Vec::new(),
        }
    }
}

impl GluePreview {
    /// The create screen's stage: the race's scene, fogged as the create screen fogs it, with the
    /// dialled look standing in it.
    pub(crate) fn show_create(&mut self, race: u8, look: CreateLook) {
        let scene = GlueScene::of_race(race);
        self.scene = Some(scene);
        self.look = Some(GlueLook::Create(look));
        self.fog = Some(scene.stock_fog());
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
    /// The fog the spawn wrote; the scene rebuilds when it changes.
    fog: Option<SceneFog>,
    /// The [`GluePreview::stage`] the spawn stood; the scene rebuilds when it changes.
    stage: u64,
    /// Whether the buffer holds the ghost rig ([`ghost_rig`]); a flip rewrites the buffer in
    /// place, like the reference's per-frame fill callback, so the stage's emitters keep running.
    ghost: bool,
    /// The scene's light buffer: its authored rig ([`SceneRig`]) plus [`GluePreview::fog`].
    light: Option<bevy::render::render_resource::Buffer>,
    /// Booth twins of the character's materials against the scene buffer (`BoothLight::variants`).
    variants: std::collections::HashMap<AssetId<WowModelMaterial>, Handle<WowModelMaterial>>,
    /// Bumped on every scene spawn; `sync_glue_booth` keys on it to re-light the character.
    rev: u64,
    /// The requested stage not yet committed ([`PendingSwap`]).
    pending: Option<PendingSwap>,
    /// The stage's scene clock in milliseconds, the reference's per-`ModelFFX` scene clock
    /// (`CSimpleModel+0x314`): it steps in whole milliseconds, only while the pane shows
    /// ([`pace_glue_clock`]).
    clock_ms: u64,
    /// The scenes left for another pane's while their own pane still holds them
    /// ([`GluePreview::live`]), each to resume where it stopped when its stage returns.
    parked: Vec<ParkedScene>,
    /// Scene roots gone since [`sync_glue_scene`] last looked, whose own materials it forgets
    /// (`M2BatchMaterials::forget_scene`).
    dropped: Vec<Entity>,
}

/// A scene left standing for another pane's, its pane still holding it: a hidden `ModelFFX` keeps
/// its model and scene, and only its clock stops (`0x76d7f0`). The login screen's `UI_MainMenu` is
/// the case: `AccountLogin.xml`'s `file=` is its only `SetModel` (the attribute handler
/// `0x76cce0`), so its scene outlives every visit to the other screens. Its root is hidden, its
/// pose held, its players at speed 0 and its emitters frozen; its clock and its materials' clock
/// keep the values they stopped at.
struct ParkedScene {
    root: Entity,
    token: &'static str,
    handle: Handle<M2Model>,
    cam: Option<benilla_assets::PortraitCamera>,
    char_spot: Vec3,
    char_facing: Quat,
    char_scale: f32,
    pet_spot: Vec3,
    pet_facing: Quat,
    pet_stage_scale: f32,
    stage: u64,
    clock_ms: u64,
}

impl CreateScene {
    /// No scene up, over its two roots.
    fn empty(root: Entity, pet_root: Entity) -> Self {
        CreateScene {
            root,
            token: None,
            handle: None,
            spawned: false,
            cam: None,
            char_spot: Vec3::ZERO,
            char_facing: Quat::IDENTITY,
            char_scale: 1.0,
            pet_root,
            pet_spot: Vec3::ZERO,
            pet_facing: Quat::IDENTITY,
            pet_stage_scale: 1.0,
            pet_baked: None,
            fog: None,
            stage: 0,
            ghost: false,
            light: None,
            variants: default(),
            rev: 0,
            pending: None,
            clock_ms: 0,
            parked: Vec::new(),
            dropped: Vec::new(),
        }
    }

    /// Step off the standing scene for stage `next`: park it while its pane still holds it
    /// (`live`), else strip its root; then stand `next` back up if it is parked, returning whether
    /// it was, so the caller lights it instead of spawning. `children` lists an entity's children,
    /// for the emitters to freeze and thaw. Token and handle stay for the caller to set on a
    /// spawn; a resumed scene brings its own.
    fn leave_for(
        &mut self,
        commands: &mut Commands,
        layer: &RenderLayers,
        live: &[u64],
        next: u64,
        children: &dyn Fn(Entity) -> Vec<Entity>,
    ) -> bool {
        let returning = self
            .parked
            .iter()
            .position(|p| p.stage == next)
            .map(|i| self.parked.remove(i));
        let parks = self.spawned && self.stage != next && live.contains(&self.stage);
        match (parks, self.token, self.handle.clone()) {
            (true, Some(token), Some(handle)) => {
                hold_scene(commands, self.root, true, children);
                self.parked.push(ParkedScene {
                    root: self.root,
                    token,
                    handle,
                    cam: self.cam,
                    char_spot: self.char_spot,
                    char_facing: self.char_facing,
                    char_scale: self.char_scale,
                    pet_spot: self.pet_spot,
                    pet_facing: self.pet_facing,
                    pet_stage_scale: self.pet_stage_scale,
                    stage: self.stage,
                    clock_ms: self.clock_ms,
                });
            }
            _ => {
                // Gone for good, its root and its own materials with it.
                commands.entity(self.root).despawn();
                self.dropped.push(self.root);
            }
        }
        // A fresh root for the incoming spawn, whose materials are its own; a resumed scene
        // brings its root back.
        if returning.is_none() {
            self.root = commands.spawn(stage_root(layer)).id();
        }
        self.spawned = false;
        self.cam = None;
        self.char_spot = Vec3::ZERO;
        let Some(p) = returning else {
            // A new instance's clock starts from 0, as the widget scene's constructor zeroes it
            // (`0x706ff0`): its sequence bands then sample `clock − lo` (`0x714530`) from an arm
            // at 0, and its global sequences `clock − attach`.
            self.clock_ms = 0;
            return false;
        };
        hold_scene(commands, p.root, false, children);
        self.root = p.root;
        self.token = Some(p.token);
        self.handle = Some(p.handle);
        self.cam = p.cam;
        self.char_spot = p.char_spot;
        self.char_facing = p.char_facing;
        self.char_scale = p.char_scale;
        self.pet_spot = p.pet_spot;
        self.pet_facing = p.pet_facing;
        self.pet_stage_scale = p.pet_stage_scale;
        self.stage = p.stage;
        self.clock_ms = p.clock_ms;
        self.spawned = true;
        true
    }

    /// Off the glue screens: drop every scene, parked or standing, and the relit twins of the
    /// models that stood (`variants`), so nothing holds their materials and the tick frees their
    /// rows. Returns whether a scene stood, for the caller to square its target back.
    fn stand_down(&mut self, commands: &mut Commands) -> bool {
        self.evict(commands, &[]);
        self.variants.clear();
        if self.token.is_none() {
            return false;
        }
        commands.entity(self.root).despawn_related::<Children>();
        self.dropped.push(self.root);
        // Strip the rig state (pose buffer, player, palette slot): the root outlives its bakes.
        super::clear_booth_rig(commands, self.root);
        self.token = None;
        self.handle = None;
        self.spawned = false;
        self.cam = None;
        self.char_spot = Vec3::ZERO;
        self.char_facing = Quat::IDENTITY;
        self.char_scale = 1.0;
        clear_pet(commands, self);
        true
    }

    /// Drop every parked scene whose pane let it go: a new `SetModel` there (`0x76cce0`), or the
    /// glue screens gone.
    fn evict(&mut self, commands: &mut Commands, live: &[u64]) {
        let dropped = &mut self.dropped;
        self.parked.retain(|p| {
            let keep = live.contains(&p.stage);
            if !keep {
                commands.entity(p.root).despawn();
                dropped.push(p.root);
            }
            keep
        });
    }
}

/// Hold a scene's root still and out of sight, or release it: hidden, its pose evaluation parked,
/// its players at speed 0 ([`pace_glue_clock`] sets the standing scene's each frame) and its
/// emitters, the root's children, frozen.
fn hold_scene(
    commands: &mut Commands,
    root: Entity,
    held: bool,
    children: &dyn Fn(Entity) -> Vec<Entity>,
) {
    let mut model = commands.entity(root);
    if held {
        model
            .insert((Visibility::Hidden, benilla_world::rig_anim::AnimParked))
            .entry::<AnimationPlayer>()
            .and_modify(|mut player| {
                for (_, active) in player.playing_animations_mut() {
                    active.set_speed(0.0);
                }
            });
    } else {
        model
            .insert(Visibility::Visible)
            .remove::<benilla_world::rig_anim::AnimParked>();
    }
    for child in children(root) {
        commands
            .entity(child)
            .entry::<benilla_world::particles::ParticleEmitter>()
            .and_modify(move |mut emitter| emitter.set_frozen(held));
    }
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

/// Whether the stage may swap this frame: nothing coherent is `standing`, both the requested art
/// and body are ready, or the hold has reached [`GLUE_SWAP_HOLD_SECS`].
fn may_swap(standing: bool, art_ready: bool, body_ready: bool, waited: f32) -> bool {
    !standing || (art_ready && body_ready) || waited >= GLUE_SWAP_HOLD_SECS
}

/// A glue scene's authored M2 light rig, folded as the reference's gather (`0x718960`):
/// directional ambients sum, each directional's diffuse × intensity becomes an SH lobe
/// ([`benilla_world::lighting::prop_probe_coeffs`]), and point lights go to the point table with
/// the fixed falloff `1/(0.7·d + 0.03·d²)`; the reference ignores the authored attenuation radii.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SceneRig {
    /// The summed directional ambient (every UI_* point light authors ambient ×0).
    pub(crate) ambient: [f32; 3],
    /// The directional lobes, toward-light and committed; folded into probe slot 0.
    pub(crate) lobes: Vec<(Vec3, [f32; 3])>,
    /// The point lights: Bevy position and committed colour.
    pub(crate) points: Vec<(Vec3, [f32; 3])>,
}

/// Range packed with a scene point light: the reference gather has no range gate (≤3 nearest).
const SCENE_POINT_RANGE: f32 = 1.0e6;

/// Fold the scene model's lights into a [`SceneRig`]. A directional's to-light direction is its
/// bone's local +Z, row 2 of the light bone's pose matrix (`0x718a76`), never the def position.
pub(crate) fn scene_rig(lights: &[benilla_assets::ModelLight]) -> SceneRig {
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
fn scene_light_blob(rig: &SceneRig, fog: Option<SceneFog>) -> benilla_world::lighting::LightBlob {
    let span = fog.unwrap_or(SceneFog {
        rgb: [0.0; 3],
        near: 0.0,
        far: 1.0,
    });
    let mut blob = benilla_world::lighting::LightBlob::model(rig.ambient, [0.0; 3], Vec3::NEG_Y)
        .probe(rig.ambient, &rig.lobes)
        .fog_span(span.rgb, span.near, span.far, fog.is_some())
        .dial(1.0);
    for (pos, color) in &rig.points {
        blob = blob.point(*pos, SCENE_POINT_RANGE, *color);
    }
    blob
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
    pub(crate) rig: Option<super::EffectRig>,
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
    let root = commands.spawn(stage_root(&layer)).id();
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
        // `gate_booth_cameras` keeps the glue camera live while a scene shows.
        Booth::new(layer.clone(), root, image),
    );
    // The background scene's own root; the character root yaws, this one never does.
    let scene_root = commands.spawn(stage_root(&layer)).id();
    // The pet's root never yaws: the reference's facing call turns only the character (`0x4730e0`).
    let pet_root = commands.spawn(stage_root(&layer)).id();
    commands.insert_resource(CreateScene::empty(scene_root, pet_root));
}

/// A model root on the glue stage, its scene clock at 0 from birth (the widget scene's constructor
/// zeroes it, `0x706ff0`), so no reader latches an anchor off the world's clock before
/// [`pace_glue_clock`] first writes it.
fn stage_root(layer: &RenderLayers) -> impl Bundle {
    (
        Transform::IDENTITY,
        Visibility::Visible,
        layer.clone(),
        benilla_world::rig_anim::SceneClock(0.0),
    )
}

/// Drive the glue FFX state ([`benilla_world::ffx_glow::GlueFfx`]) off the live look: the pass
/// installed and the pinned `LightParams.glow`.
///
/// | look | reference | pass | glow |
/// |---|---|---|---|
/// | none, from the show (login) | `CSimpleModelFFX::OnShow 0x46fa60` | glow | `*0x8380b4` 0.30 |
/// | a create look | same, no select build runs | glow | 0.30 |
/// | a living select record | `0x472ff7`/`0x473002` | glow | `*0x838574` 0.40 |
/// | a ghost select record | `0x472fde`/`0x472fe9` | death | `*0x838570` 0.15 |
///
/// Only the OnShow and a select build write it. A select scene with nobody standing keeps what
/// the last build wrote: the list ask (`0x4723d0`), the teardown (`0x472580`) and the load
/// (`0x4724d0`) never touch it, and an empty account's build exits before it (`0x472950` is gated
/// `-1 ≤ [0x83856c] < [0xb42140]`). The screen's show ([`GluePreview::shown`]) puts it back.
pub(super) fn sync_glue_ffx(
    preview: Res<GluePreview>,
    mut ffx: ResMut<benilla_world::ffx_glow::GlueFfx>,
    mut shown: Local<Option<u64>>,
) {
    let held = if *shown != Some(preview.shown) {
        *shown = Some(preview.shown);
        benilla_world::ffx_glow::GlueFfx::Shown
    } else {
        *ffx
    };
    let want = glue_ffx(preview.look.as_ref(), held);
    if *ffx != want {
        *ffx = want;
    }
}

/// The glue FFX state a look leaves, over `held`, the state before it ([`sync_glue_ffx`]).
fn glue_ffx(
    look: Option<&GlueLook>,
    held: benilla_world::ffx_glow::GlueFfx,
) -> benilla_world::ffx_glow::GlueFfx {
    use benilla_world::ffx_glow::GlueFfx;
    match look {
        Some(GlueLook::Select(l)) if l.flags & benilla_protocol::CHARACTER_FLAG_GHOST != 0 => {
            GlueFfx::SelectGhost
        }
        Some(GlueLook::Select(_)) => GlueFfx::SelectLiving,
        Some(GlueLook::Create(_)) => GlueFfx::Shown,
        None => held,
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
    // The swap gate's inputs ([`PendingSwap`]), and the roots' children, where a parked scene's
    // emitters are ([`CreateScene::leave_for`]).
    swap: (
        Res<GluePreviewBake>,
        Res<Time<bevy::time::Real>>,
        Query<&Children>,
    ),
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
    let (bake, real, kids) = swap;
    let Some(booth) = booths.0.get(GLUE_SLOT) else {
        return;
    };
    let children = |e: Entity| kids.get(e).map(|c| c.to_vec()).unwrap_or_default();
    let Some(which) = preview.scene else {
        // Off the glue screens: tear the scene down, free the model, square target back.
        if scene.stand_down(&mut commands) {
            resize_target(&mut images, &booth.target, GLUE_SIZE, GLUE_SIZE);
        }
        for root in scene.dropped.drain(..) {
            mats.forget_scene(root);
        }
        return;
    };

    // The target is the window's, before a token resolves; a pane is a viewport inside it
    // ([`pillarbox_glue_scene`]).
    if let Ok(w) = window.single() {
        resize_target(
            &mut images,
            &booth.target,
            w.physical_width().max(1),
            w.physical_height().max(1),
        );
    }

    scene.evict(&mut commands, &preview.live);
    let token = which.token();
    let fog = preview.fog;
    // The ghost light fork (`0x472150`, [`ghost_rig`]), shared by the scene, character and pet.
    let ghost = matches!(
        preview.look,
        Some(GlueLook::Select(l)) if l.flags & benilla_protocol::CHARACTER_FLAG_GHOST != 0
    );
    if scene.token == Some(token) {
        // The requested stage is already standing: no swap pends.
        scene.pending = None;
    }
    // A parked scene that stands again ([`CreateScene::leave_for`]).
    let mut resumed = false;
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
        clear_pet(&mut commands, &mut scene);
        resumed = scene.leave_for(
            &mut commands,
            &booth.layer,
            &preview.live,
            preview.stage,
            &children,
        );
        if !resumed {
            scene.token = Some(token);
            scene.handle = Some(pending.handle);
        }
    } else if scene.spawned && scene.stage != preview.stage {
        // Same file, another instance: this pane's new one or another pane's (the handle is
        // cached, so a spawn is now).
        clear_pet(&mut commands, &mut scene);
        resumed = scene.leave_for(
            &mut commands,
            &booth.layer,
            &preview.live,
            preview.stage,
            &children,
        );
    } else if scene.spawned && scene.fog != fog {
        // Same scene, other screen: rebuild in place.
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
            scene_light_blob(&rig, fog).write(&queue, &light);
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
    // The scenes gone this frame take their own materials with them.
    for root in scene.dropped.drain(..) {
        mats.forget_scene(root);
    }
    if resumed {
        // The shared light buffer carries the standing scene's rig and fog: put this one's back.
        if let (Some(model), Some(light)) = (
            scene.handle.as_ref().and_then(|h| m2s.get(h)),
            scene.light.clone(),
        ) {
            let rig = if ghost {
                ghost_rig()
            } else {
                scene_rig(&model.lights)
            };
            scene_light_blob(&rig, fog).write(&queue, &light);
        }
        scene.fog = fog;
        scene.ghost = ghost;
        // The character and the pet re-seat and re-light on the stage that came back.
        scene.pet_baked = None;
        scene.rev += 1;
        info!(
            "glue scene: UI_{token} resumed at {} ms on its own clock",
            scene.clock_ms
        );
        apply_yaw(
            &mut commands,
            booth.root,
            scene.char_spot,
            scene.char_facing,
            scene.char_scale,
            preview.yaw,
        );
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
        let blob = scene_light_blob(&rig, fog);
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
        scene.stage = preview.stage;
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
        let scene_root = scene.root;
        let scene_parts: Vec<BoothPart> = model
            .submeshes
            .iter()
            .enumerate()
            .map(|(pi, s)| {
                // The authored batch order (index + 1) is all that separates batches sharing the
                // scene root's sort distance.
                let order = u16::try_from(pi + 1).unwrap_or(u16::MAX);
                // The scene's own material, never another scene's of the same file: its UV and
                // tint rows run on this scene's clock and go with it.
                let material = mats.off_world_for(s, s.texture.clone(), order, &light, scene_root);
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
                // Both lanes run on the stage's own clock ([`pace_glue_clock`]), which stops
                // while the pane is hidden, as every model in a `ModelFFX`'s scene does.
                if mat_anim {
                    benilla_world::doodad_anim::scene_clocked(
                        &mut uv_reg,
                        &mut tint_reg,
                        material.id(),
                        scene_root,
                    );
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
        // The record fov is diagonal, converted at the pane's own aspect as the reference does.
        // Far is kept generous (27.8 authored on Orc).
        let pane = window
            .single()
            .ok()
            .zip(preview.pane)
            .and_then(|(w, pane)| stock_pane_view(w.physical_width(), w.physical_height(), pane));
        let vert_fov = scene_vert_fov(cam.fov, aspect, pane);
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

/// The scene camera's vertical opening for its record's diagonal `fov` by the reference's law
/// (`0x5c3cc0`): at a stock pane's own aspect, else, with no pane laid out, the window's, as a
/// fullscreen `ModelFFX` fills it (`0x76d42d`).
fn scene_vert_fov(fov: f32, window_aspect: f32, pane: Option<super::framing::PaneView>) -> f32 {
    diag_to_vert(fov, pane.map_or(window_aspect, |view| view.aspect))
}

/// Set the glue booth camera's viewport: a stock pane's rect ([`stock_pane_view`]), else the whole
/// window; writes only on change.
pub(super) fn pillarbox_glue_scene(
    preview: Res<GluePreview>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut cams: Query<(&BoothCam, &mut Camera)>,
) {
    let Ok(w) = window.single() else {
        return;
    };
    let full_h = w.physical_height().max(1);
    let viewport = preview.pane.and_then(|pane| {
        stock_pane_view(w.physical_width(), full_h, pane).map(|view| bevy::camera::Viewport {
            physical_position: view.position,
            physical_size: view.size,
            ..default()
        })
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

/// The material lanes a stage model's own rows go on ([`benilla_world::doodad_anim::clocked_twin`]).
type StageLanes<'w> = (
    ResMut<'w, benilla_world::doodad_anim::UvAnimMaterials>,
    ResMut<'w, benilla_world::doodad_anim::TintAnimMaterials>,
    ResMut<'w, benilla_world::mat_anim_table::MatAnimTable>,
);

/// What [`sync_glue_booth`] keeps between frames.
#[derive(Default)]
pub(super) struct GlueStanding {
    /// The bake revision, yaw and scene revision last committed.
    last: Option<(u64, f32, u64)>,
    /// The create character standing as one instance a re-dress keeps. `None` for a select
    /// record, an empty stage or a boneless bake.
    create: Option<StandingCreate>,
}

/// The create body standing: one `CharacterModel` while its display and its stage last.
struct StandingCreate {
    /// The race and sex body: a change is the builder's new model (`0x470b70`).
    display: u32,
    /// The stage it was built on ([`GluePreview::stage`]): every create `OnShow` builds anew, its
    /// `SetCharCustomizeBackground` making a new scene (`0x4707b0` → `0x76cce0`, unconditional) and
    /// `ResetCharCustomize` a new body (`0x4709f3`).
    stage: u64,
    /// Its attachments as hung.
    hung: Hung,
}

/// The attachments a bake hung, by what each draws and where: a re-dress re-hangs them only when
/// this changes, so a customization leaves the worn items' models and emitters running, as the
/// reference's setters touch only the body (`0x478790`), while a class's new outfit re-hangs them
/// (`0x470800`'s unequip `0x478960` and equip `0x479010`).
#[derive(PartialEq)]
struct Hung {
    riders: Vec<HungSeat>,
    cards: Vec<(HungSeat, benilla_formats::BillboardKind)>,
    /// Each effect model by its seat and emitter count; its geometry is among the riders.
    effects: Vec<(u16, [u32; 3], usize)>,
    /// The light buffer the effects bind ([`super::booth::spawn_booth_effects`]).
    light: Option<bevy::render::render_resource::BufferId>,
}

/// One hung model: its mesh, its material, its bone and its offset's bits.
type HungSeat = (AssetId<Mesh>, AssetId<WowModelMaterial>, u16, [u32; 3]);

impl Hung {
    fn of(
        riders: &[BoothRider],
        cards: &[BoothBillboardSpec],
        effects: &[BoothEffects],
        light: Option<&bevy::render::render_resource::Buffer>,
    ) -> Self {
        let at = |v: Vec3| v.to_array().map(f32::to_bits);
        Hung {
            riders: riders
                .iter()
                .map(|r| (r.mesh.id(), r.material.id(), r.bone, at(r.offset)))
                .collect(),
            cards: cards
                .iter()
                .map(|c| ((c.mesh.id(), c.material.id(), c.bone, at(c.offset)), c.kind))
                .collect(),
            effects: effects
                .iter()
                .map(|e| (e.bone, at(e.offset), e.emitters.len()))
                .collect(),
            light: light.map(|b| b.id()),
        }
    }
}

/// The standing instance's rig on the booth root, which a re-dress keeps ([`sync_glue_booth`]):
/// its palette slot, its children, and whether its player stands. The player is reached through
/// commands, so this system shares no access with the world's animation systems.
type StandingRig<'a> = (
    &'a benilla_world::rig_palette::RigSkin,
    Option<&'a Children>,
    Has<AnimationPlayer>,
);

/// Re-bake the glue booth when the parts or the scene's rig change, relit onto the scene's rig
/// (the reference's glue character is scene-lit) or the studio buffer, and spin it to the yaw.
///
/// The create character is one model across its customizations and class re-dresses, which
/// update it in place (`0x478790` and kin, `0x470800`); only the race and sex builder `0x470b70`
/// makes a new one. So a create bake on the standing body display re-dresses the standing
/// instance ([`redress_booth_batches`]) and its clock runs on; anything else spawns afresh.
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
    standing_rigs: Query<StandingRig>,
    batches: Query<&Mesh3d, With<BoothBatch>>,
    mut state: Local<GlueStanding>,
    mut lanes: StageLanes,
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
    let (last_rev, last_yaw, last_scene) = state.last.unwrap_or((u64::MAX, f32::NAN, u64::MAX));
    // `sync_glue_scene` runs first and commits a stage only beside its character, so a standing
    // stage matching the request means this bake may show ([`PendingSwap`]).
    let staged = scene
        .as_deref()
        .is_none_or(|s| s.token == preview.scene.map(GlueScene::token));
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
            state.last = Some((bake.revision, preview.yaw, scene_rev));
            state.create = None;
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
        let mut booth_riders: Vec<BoothRider> = bake
            .riders
            .iter()
            .map(|r| BoothRider {
                mesh: r.mesh.clone(),
                material: relight(&r.material, &mut scene, &mut booth_light, &mut materials),
                bone: r.bone,
                offset: r.offset,
                twins: relight_twins(&r.twins, &mut scene, &mut booth_light, &mut materials),
                mat_anim: false,
            })
            .collect();
        let mut booth_billboards: Vec<BoothBillboardSpec> = bake
            .billboards
            .iter()
            .map(|b| BoothBillboardSpec {
                mesh: b.mesh.clone(),
                material: relight(&b.material, &mut scene, &mut booth_light, &mut materials),
                bone: b.bone,
                offset: b.offset,
                kind: b.kind,
                twins: relight_twins(&b.twins, &mut scene, &mut booth_light, &mut materials),
                mat_anim: false,
            })
            .collect();
        // On the stage, each animated material runs on the stage's clock, following the body.
        if scene_buf.is_some() {
            let (uv, tint, table) = &mut lanes;
            let mut on_clock = |source: &Handle<WowModelMaterial>,
                                twin: &Handle<WowModelMaterial>,
                                blend: (
                Option<&Handle<WowModelMaterial>>,
                Option<&Handle<WowModelMaterial>>,
            )| {
                let steady = benilla_world::doodad_anim::clocked_twin(
                    uv,
                    tint,
                    table,
                    &mut materials,
                    source.id(),
                    twin.id(),
                    booth.root,
                );
                let feathered = match blend {
                    (Some(source), Some(twin)) => benilla_world::doodad_anim::clocked_twin(
                        uv,
                        tint,
                        table,
                        &mut materials,
                        source.id(),
                        twin.id(),
                        booth.root,
                    ),
                    _ => false,
                };
                steady || feathered
            };
            for (p, bp) in bake.parts.iter().zip(booth_parts.iter_mut()) {
                bp.mat_anim = on_clock(
                    &p.material,
                    &bp.material,
                    (p.twins.blend.as_ref(), bp.twins.blend.as_ref()),
                );
            }
            for (r, br) in bake.riders.iter().zip(booth_riders.iter_mut()) {
                br.mat_anim = on_clock(
                    &r.material,
                    &br.material,
                    (r.twins.blend.as_ref(), br.twins.blend.as_ref()),
                );
            }
            for (b, bb) in bake.billboards.iter().zip(booth_billboards.iter_mut()) {
                bb.mat_anim = on_clock(
                    &b.material,
                    &bb.material,
                    (b.twins.blend.as_ref(), bb.twins.blend.as_ref()),
                );
            }
        }
        let booth_effects: Vec<BoothEffects> = bake
            .effects
            .iter()
            .map(|fx| BoothEffects {
                bone: fx.bone,
                offset: fx.offset,
                emitters: fx.emitters.clone(),
                rig: fx.rig.clone(),
            })
            .collect();
        let hung = Hung::of(
            &booth_riders,
            &booth_billboards,
            &booth_effects,
            scene_buf.as_ref(),
        );
        // The standing create body, kept when this bake wears the same display on the same stage
        // ([`StandingCreate`]).
        let create = matches!(bake.look, Some(GlueLook::Create(_)));
        let keep = create
            && !instance.feathering()
            && state
                .create
                .as_ref()
                .is_some_and(|c| c.display == bake.display_id && c.stage == preview.stage);
        let standing = if keep {
            standing_rigs
                .get(booth.root)
                .ok()
                .filter(|(_, _, player)| *player)
        } else {
            None
        };
        // `WOW_BOOTH_LOG=1`: which display was baked, whether the look or the scene rig
        // triggered it, and whether the standing instance was kept.
        if super::booth_log() {
            println!(
                "[booth] glue rebake display={} parts={} riders={} bake_rev={} scene_rev={scene_rev} by={} {}",
                bake.display_id,
                bake.parts.len(),
                bake.riders.len(),
                bake.revision,
                if last_rev != bake.revision { "look" } else { "scene" },
                if standing.is_some() { "redress" } else { "fresh" },
            );
        }
        let slot = match standing {
            Some((skin, children, _)) => {
                let slot = skin.slot;
                let kids: Vec<Entity> = children.map(|c| c.to_vec()).unwrap_or_default();
                let worn: Vec<(Entity, AssetId<Mesh>)> = kids
                    .iter()
                    .filter_map(|&c| batches.get(c).ok().map(|m| (c, m.0.id())))
                    .collect();
                redress_booth_batches(
                    &mut commands,
                    booth.root,
                    &booth.layer,
                    &booth_parts,
                    &worn,
                    slot,
                    instance,
                );
                if state.create.as_ref().is_none_or(|c| c.hung != hung) {
                    // The outfit changed: the old items come off with their anchors, and the new
                    // ones hang on a fresh pose buffer the root's player re-poses before it draws.
                    for c in kids.iter().filter(|c| !worn.iter().any(|(w, _)| w == *c)) {
                        commands.entity(*c).despawn();
                    }
                    let mut booth_rig = BoothRig::rehang(booth.root, rig.skeleton, slot);
                    booth_rig.hang(
                        &mut commands,
                        &booth.layer,
                        &booth_riders,
                        &booth_billboards,
                        instance,
                    );
                    spawn_glue_effects(
                        &mut commands,
                        &mut booth_rig,
                        &booth.layer,
                        scene_buf.as_ref(),
                        &booth_effects,
                        instance,
                    );
                    booth_rig.finish(&mut commands);
                }
                // A class's weapons close or open its hands on the same model (`0x479660`).
                if let Some(hands) = rig.animations.map(|a| a.hand_close) {
                    let grip = bake.grip;
                    commands
                        .entity(booth.root)
                        .entry::<AnimationPlayer>()
                        .and_modify(move |mut player| set_grip(&mut player, hands, grip));
                }
                slot
            }
            None => {
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
                spawn_glue_effects(
                    &mut commands,
                    &mut booth_rig,
                    &booth.layer,
                    scene_buf.as_ref(),
                    &booth_effects,
                    instance,
                );
                // A fresh bake is animated by construction; the park state is the new rig's.
                booth.rigged = booth_rig.rigged();
                let slot = booth_rig.slot();
                booth_rig.finish(&mut commands);
                booth.parked = false;
                slot
            }
        };
        // The whole-model tint ([`GhostKit::tint`], the reference's `0x472939` → `0x710cf0`),
        // per rig slot, identity when living so a ghost's colour never carries over.
        tints.set(
            slot,
            match bake.ghost {
                Some(g) => benilla_world::instance_tint::pack(g.tint),
                None => benilla_world::instance_tint::IDENTITY,
            },
        );
        state.create = (create && booth.rigged).then_some(StandingCreate {
            display: bake.display_id,
            stage: preview.stage,
            hung,
        });
        // The fallback framing with no scene up: the model's `<PlayerModel>` pane camera, at the
        // bake's aspect, as it shows only for the frames before the scene's camera takes over.
        let (t, _) = body_frame(&anchors, 1.0);
        let record_fov = anchors
            .raw_camera(1)
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
    state.last = Some((
        if rebake { bake.revision } else { last_rev },
        preview.yaw,
        if rebake { scene_rev } else { last_scene },
    ));
}

/// The worn items' effects, through the reference's world attach primitive (`0x472c91` →
/// `0x47a0c0` → `0x4798c0`); the enum carries no enchant, so only intrinsic effects show.
fn spawn_glue_effects(
    commands: &mut Commands,
    booth_rig: &mut BoothRig,
    layer: &RenderLayers,
    light: Option<&bevy::render::render_resource::Buffer>,
    effects: &[BoothEffects],
    instance: BoothInstance,
) {
    let (fx_emitters, fx_frames) =
        spawn_booth_effects(commands, booth_rig, layer, light, effects, instance);
    if fx_emitters > 0 {
        info!(
            "glue booth: {fx_emitters} item emitter(s) up on {} effect model(s), \
             {fx_frames} on a billboard frame",
            effects.len()
        );
    }
}

/// Pace the stage's one scene clock ([`CreateScene::clock_ms`]) as a `ModelFFX` paces its own:
/// its `OnUpdate` (`0x76d7f0`), which only the visible-frame pump (`0x765650`) calls, adds the
/// frame's elapsed truncated to whole milliseconds ([`benilla_ui::widget::pane_clock_step_ms`]),
/// the step the stock panes' clocks take too. So a hidden pane's stage stands still and resumes
/// where it stopped, and a shown one runs at or below wall clock, never above. Every model on the
/// stage, the scene, the character and the pet, reads that clock
/// ([`benilla_world::rig_anim::SceneClock`] on each root): each player advances by the step, and
/// each global-sequence drive and the scene's texture transforms and colour tracks sample it.
pub(super) fn pace_glue_clock(
    mut commands: Commands,
    preview: Res<GluePreview>,
    time: Res<Time>,
    booths: Res<Booths>,
    scene: Option<ResMut<CreateScene>>,
) {
    let Some(mut scene) = scene else {
        return;
    };
    let delta = time.delta_secs();
    let step_ms = if preview.hidden {
        0
    } else {
        benilla_ui::widget::pane_clock_step_ms(delta)
    };
    scene.clock_ms += step_ms;
    let clock = benilla_world::rig_anim::SceneClock(scene.clock_ms as f64 / 1000.0);
    // A player advances by `delta · speed` (`advance_animations`), so this speed makes it step by
    // the clock's own whole milliseconds, and 0 holds it.
    let speed = if delta > 0.0 {
        step_ms as f32 / 1000.0 / delta
    } else {
        0.0
    };
    // A hidden pane's scene and pet hold their poses too: nothing it draws moves, so nothing is
    // evaluated (the character's root parks with its camera, `gate_booth_cameras`).
    for root in [scene.root, scene.pet_root] {
        if preview.hidden {
            commands
                .entity(root)
                .insert(benilla_world::rig_anim::AnimParked);
        } else {
            commands
                .entity(root)
                .remove::<benilla_world::rig_anim::AnimParked>();
        }
    }
    let character = booths.0.get(GLUE_SLOT).map(|b| b.root);
    // Through commands, not a query: the stage's three roots are all it touches, and a query of
    // every player would share its access with each world animation system. Each root carries the
    // stage's clock, which its global sequences and its materials read; a parked scene's root
    // keeps the value it stopped at.
    for root in [Some(scene.root), character, Some(scene.pet_root)]
        .into_iter()
        .flatten()
    {
        commands
            .entity(root)
            .insert(clock)
            .entry::<AnimationPlayer>()
            .and_modify(move |mut player| {
                for (_, active) in player.playing_animations_mut() {
                    active.set_speed(speed);
                }
            });
    }
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
    mut lanes: StageLanes,
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
    let mut booth_parts: Vec<BoothPart> = pet
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
    let mut booth_billboards: Vec<BoothBillboardSpec> = pet
        .billboards
        .iter()
        .map(|b| BoothBillboardSpec {
            mesh: b.mesh.clone(),
            material: relight(&b.material, &mut scene.variants),
            bone: b.bone,
            offset: b.offset,
            kind: b.kind,
            twins: BoothTwins::default(),
            mat_anim: false,
        })
        .collect();
    // The pet's animated materials run on the stage's clock, following the pet.
    let (uv, tint, table) = &mut lanes;
    let pet_root = scene.pet_root;
    let mut on_clock = |source: &Handle<WowModelMaterial>, twin: &Handle<WowModelMaterial>| {
        benilla_world::doodad_anim::clocked_twin(
            uv,
            tint,
            table,
            &mut materials,
            source.id(),
            twin.id(),
            pet_root,
        )
    };
    for (p, bp) in pet.parts.iter().zip(booth_parts.iter_mut()) {
        bp.mat_anim = on_clock(&p.material, &bp.material);
    }
    for (b, bb) in pet.billboards.iter().zip(booth_billboards.iter_mut()) {
        bb.mat_anim = on_clock(&b.material, &bb.material);
    }
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
    preview.show_create(look.race, look);
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

    /// `GlueParent`'s rect on a window of `aspect` (`GlueParent.lua:78-84`): the whole screen up to
    /// 16:9, a centred 16:9 box past it; as window fractions from the top-left.
    fn glue_parent(aspect: f32) -> [f32; 4] {
        let bar = ((aspect - 16.0 / 9.0) / (2.0 * aspect)).max(0.0);
        [bar, 1.0 - bar, 0.0, 1.0]
    }

    const WINDOWS: [(u32, u32); 4] = [(1024, 768), (1280, 800), (1920, 1080), (2560, 1080)];

    /// Under the stock screens the scene fills the pane, `GlueParent`'s rect, at the pane's
    /// aspect, `min(window, 16:9)`, through the diagonal law: no box inside it, and the only bars
    /// are outside `GlueParent`, at 21:9.
    #[test]
    fn a_stock_pane_frames_the_scene_at_its_own_aspect() {
        let fov = 1.0;
        for (w, h) in WINDOWS {
            let a = w as f32 / h as f32;
            let view = super::super::framing::stock_pane_view(w, h, glue_parent(a)).unwrap();
            let pane_w = (h as f32 * a.min(16.0 / 9.0)).round() as u32;
            assert_eq!(view.size, UVec2::new(pane_w, h), "{w}x{h}");
            assert_eq!(view.position, UVec2::new((w - pane_w) / 2, 0), "{w}x{h}");
            let want = a.min(16.0 / 9.0);
            assert!(
                (view.aspect - want).abs() < 1e-3,
                "{w}x{h}: {}",
                view.aspect
            );
            let vert = scene_vert_fov(fov, a, Some(view));
            let law = fov / (want * want + 1.0).sqrt();
            assert!((vert - law).abs() < 1e-3, "{w}x{h}: {vert} vs {law}");
        }
        // With no pane, the window's own aspect by the same law.
        let window = scene_vert_fov(fov, 1.6, None);
        assert!((window - fov / (1.6f32 * 1.6 + 1.0).sqrt()).abs() < 1e-3);
    }

    /// The booth camera's viewport is the stock pane's rect, no pillarbox inside it, and with no
    /// pane the whole window.
    #[test]
    fn the_viewport_is_the_stock_pane_or_the_window() {
        for stock in [true, false] {
            for (w, h) in WINDOWS {
                let a = w as f32 / h as f32;
                let mut app = App::new();
                let preview = GluePreview {
                    pane: stock.then(|| glue_parent(a)),
                    ..GluePreview::default()
                };
                app.insert_resource(preview)
                    .add_systems(Update, pillarbox_glue_scene);
                app.world_mut().spawn((
                    Window {
                        resolution: UVec2::new(w, h).into(),
                        ..default()
                    },
                    PrimaryWindow,
                ));
                let cam = app
                    .world_mut()
                    .spawn((BoothCam(GLUE_SLOT.to_string()), Camera::default()))
                    .id();
                app.update();
                let vp = app
                    .world()
                    .get::<Camera>(cam)
                    .unwrap()
                    .viewport
                    .clone()
                    .map(|v| (v.physical_position, v.physical_size));
                let want = if stock {
                    let view = super::super::framing::stock_pane_view(w, h, glue_parent(a));
                    view.map(|v| (v.position, v.size))
                } else {
                    None
                };
                assert_eq!(vp, want, "stock {stock} {w}x{h}");
                if stock && a <= 16.0 / 9.0 {
                    assert_eq!(
                        vp,
                        Some((UVec2::ZERO, UVec2::new(w, h))),
                        "no bars at {w}x{h}"
                    );
                }
            }
        }
    }

    /// The glow a look leaves: a select build writes its own, and nobody standing keeps what the
    /// last build wrote until the screen's show puts the OnShow 0.30 back.
    #[test]
    fn an_empty_select_holds_the_last_builds_glow_until_the_screen_shows() {
        use benilla_world::ffx_glow::GlueFfx;
        let mut app = App::new();
        app.init_resource::<GluePreview>()
            .init_resource::<GlueFfx>()
            .add_systems(Update, sync_glue_ffx);
        let mut ghost = benilla_protocol::Character {
            flags: benilla_protocol::CHARACTER_FLAG_GHOST,
            ..crate::glue_session::roster::test_character(1, "Boo")
        };
        let set = |app: &mut App, look: Option<GlueLook>, show: bool| {
            {
                let mut p = app.world_mut().resource_mut::<GluePreview>();
                p.look = look;
                if show {
                    p.shown += 1;
                }
            }
            app.update();
            *app.world().resource::<GlueFfx>()
        };
        assert_eq!(set(&mut app, None, true), GlueFfx::Shown);
        let ghost_look = GlueLook::Select(SelectLook::from(&ghost));
        assert_eq!(set(&mut app, Some(ghost_look), false), GlueFfx::SelectGhost);
        assert_eq!(
            set(&mut app, None, false),
            GlueFfx::SelectGhost,
            "a reload keeps it"
        );
        ghost.flags = 0;
        let living = GlueLook::Select(SelectLook::from(&ghost));
        assert_eq!(set(&mut app, Some(living), false), GlueFfx::SelectLiving);
        assert_eq!(set(&mut app, None, false), GlueFfx::SelectLiving);
        assert_eq!(
            set(&mut app, None, true),
            GlueFfx::Shown,
            "the show puts 0.30 back"
        );
        // A build in the show's own frame wins: it runs after the OnShow.
        assert_eq!(set(&mut app, Some(living), true), GlueFfx::SelectLiving);
    }

    /// A scene is its model's token, from the path the glue screens give the pane in any case or
    /// extension; `SetBackgroundModel`'s race remap lands on the same scenes.
    #[test]
    fn a_scene_is_named_by_its_models_token() {
        for scene in GlueScene::ALL {
            let token = scene.token();
            let path = format!("Interface\\Glues\\Models\\UI_{token}\\UI_{token}.mdx");
            assert_eq!(GlueScene::from_model_path(&path), Some(scene), "{path}");
        }
        assert_eq!(
            GlueScene::from_model_path("interface/glues/models/ui_nightelf/UI_NIGHTELF.M2"),
            Some(GlueScene::NightElf)
        );
        for odd in [
            "",
            "UI_",
            "UI_Gnome.mdx",
            "Interface\\Glues\\Models\\Orc.mdx",
        ] {
            assert_eq!(GlueScene::from_model_path(odd), None, "{odd}");
        }
        assert_eq!(GlueScene::of_race(8), GlueScene::Orc);
        assert_eq!(GlueScene::of_race(7), GlueScene::Dwarf);
        assert_eq!(GlueScene::of_race(1), GlueScene::Human);
    }

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

    // ── The create body is one instance ───────────────────────────────────

    const HUMAN_MALE: u32 = 49;
    const HUMAN_FEMALE: u32 = 50;

    fn handle<A: Asset>(n: u128) -> Handle<A> {
        Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(n),
            std::marker::PhantomData,
        )
    }

    /// Two bones, enough for a pose buffer, an anchor and a palette slot.
    fn two_bones() -> benilla_assets::ModelSkeleton {
        let joint = |parent, y| benilla_assets::ModelJoint {
            parent,
            local_translation: Vec3::Y * y,
            billboard: None,
            parent_arm: None,
        };
        benilla_assets::ModelSkeleton {
            joints: vec![joint(-1, 0.0), joint(0, 1.0)],
            spine_bone: None,
            head_bone: None,
        }
    }

    /// The node Stand plays on, and the right hand's grip overlay.
    const STAND_NODE: usize = 1;
    const GRIP_NODE: usize = 2;

    fn node(n: usize) -> bevy::animation::graph::AnimationNodeIndex {
        bevy::animation::graph::AnimationNodeIndex::new(n)
    }

    fn body_anims() -> benilla_assets::ModelAnimations {
        benilla_assets::ModelAnimations {
            graph: Handle::default(),
            clips: vec![benilla_assets::AnimClip {
                anim_id: 0,
                seq_index: 0,
                node: node(STAND_NODE),
                looping: true,
                duration: 2.0,
                move_speed: 0.0,
                blend_time: 0.0,
                bounds_center: Vec3::ZERO,
                bounds_radius: 0.0,
                bounds_min: Vec3::ZERO,
                bounds_max: Vec3::ZERO,
                events: Vec::new().into(),
                arm_nodes: None,
                upper_node: None,
                frequency: 0,
                replay: (0, 0),
                poses_bones: true,
            }],
            hand_close: [Some(node(GRIP_NODE)), None],
            playable_animation_lookup: Vec::new(),
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            first_seq: None,
            pose: Default::default(),
        }
    }

    fn create(sex: u8, class: u8, skin: u8, hair_style: u8) -> GlueLook {
        GlueLook::Create(CreateLook {
            race: 1,
            sex,
            class,
            skin,
            face: 0,
            hair_style,
            hair_color: 0,
            facial_hair: 0,
        })
    }

    /// One body batch: its static mesh `mesh`, its skinned twin `mesh + 1000`, and `material`.
    fn part(mesh: u128, material: u128) -> PreviewPart {
        PreviewPart {
            static_mesh: handle(mesh),
            skinned_mesh: Some(handle(mesh + 1000)),
            material: handle(material),
            alpha_anim: None,
            twins: BoothTwins::default(),
        }
    }

    /// The glue booth over the character root `root`, as [`spawn_glue_booth`] stands it.
    fn glue_booth(root: Entity) -> Booth {
        Booth::new(RenderLayers::layer(GLUE_LAYER), root, Handle::default())
    }

    fn booth_app() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<GluePreview>()
            .init_resource::<GluePreviewBake>()
            .init_resource::<BoothLight>()
            .init_resource::<Assets<WowModelMaterial>>()
            .init_resource::<benilla_world::rig_palette::RigPalettes>()
            .init_resource::<benilla_world::instance_tint::InstanceTints>()
            .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
            .init_resource::<benilla_world::doodad_anim::TintAnimMaterials>()
            .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
            .insert_resource(Creatures::rigged_for_test(vec![
                (HUMAN_MALE, two_bones(), Some(body_anims())),
                (HUMAN_FEMALE, two_bones(), Some(body_anims())),
            ]))
            .add_systems(Update, sync_glue_booth);
        let root = app.world_mut().spawn(Transform::IDENTITY).id();
        let mut booths = Booths::default();
        booths.0.insert(GLUE_SLOT.to_string(), glue_booth(root));
        app.insert_resource(booths);
        (app, root)
    }

    /// Land a bake of `look` on `display`, as the builder does, and run a frame.
    fn bake(
        app: &mut App,
        look: GlueLook,
        display_id: u32,
        parts: Vec<PreviewPart>,
        riders: Vec<PreviewRider>,
        grip: [bool; 2],
    ) {
        let mut bake = app.world_mut().resource_mut::<GluePreviewBake>();
        let revision = bake.revision + 1;
        *bake = GluePreviewBake {
            look: Some(look),
            display_id,
            parts,
            riders,
            grip,
            revision,
            ..default()
        };
        app.update();
    }

    /// The Stand clock's position on the root's player.
    fn stand_at(app: &App, root: Entity) -> f32 {
        app.world()
            .get::<AnimationPlayer>(root)
            .and_then(|p| p.animation(node(STAND_NODE)))
            .expect("Stand plays on the body")
            .seek_time()
    }

    fn run_stand_to(app: &mut App, root: Entity, t: f32) {
        app.world_mut()
            .get_mut::<AnimationPlayer>(root)
            .and_then(|mut p| {
                p.animation_mut(node(STAND_NODE)).map(|a| {
                    a.seek_to(t);
                })
            })
            .expect("Stand plays on the body");
    }

    /// The body's batches, `(entity, material)` by static mesh number.
    fn worn(app: &mut App, root: Entity) -> Vec<(u128, Entity, AssetId<WowModelMaterial>)> {
        let mut q = app.world_mut().query_filtered::<(
            Entity,
            &Mesh3d,
            &MeshMaterial3d<WowModelMaterial>,
            &ChildOf,
        ), With<BoothBatch>>();
        let mut out: Vec<_> = q
            .iter(app.world())
            .filter(|(_, _, _, c)| c.parent() == root)
            .map(|(e, m, mat, _)| {
                let AssetId::Uuid { uuid } = m.0.id() else {
                    panic!("a test mesh");
                };
                (uuid.as_u128() - 1000, e, mat.0.id())
            })
            .collect();
        out.sort_by_key(|(n, _, _)| *n);
        out
    }

    /// The create body is one `CharacterModel` across its customizations (`0x478790` and kin
    /// rebind its textures) and class re-dresses (`0x470800` re-equips it); only the race and sex
    /// builder `0x470b70` makes a new one. So the Stand clock runs on through the first two, and
    /// only a sex change starts it over.
    #[test]
    fn a_customization_or_class_change_keeps_the_create_body_and_its_clock() {
        let (mut app, root) = booth_app();
        bake(
            &mut app,
            create(0, 1, 0, 0),
            HUMAN_MALE,
            vec![part(1, 10), part(2, 11)],
            Vec::new(),
            [false, false],
        );
        run_stand_to(&mut app, root, 1.25);
        let before = worn(&mut app, root);
        assert_eq!(before.len(), 2);

        // Skin: the same batches, a recomposited body texture.
        bake(
            &mut app,
            create(0, 1, 3, 0),
            HUMAN_MALE,
            vec![part(1, 20), part(2, 21)],
            Vec::new(),
            [false, false],
        );
        assert_eq!(stand_at(&app, root), 1.25, "a skin change keeps the clock");
        let skin = worn(&mut app, root);
        assert_eq!(
            skin.iter().map(|(n, e, _)| (*n, *e)).collect::<Vec<_>>(),
            before.iter().map(|(n, e, _)| (*n, *e)).collect::<Vec<_>>(),
            "the same batch entities stand"
        );
        assert_eq!(
            skin.iter().map(|(_, _, m)| *m).collect::<Vec<_>>(),
            vec![handle::<WowModelMaterial>(20).id(), handle(21).id()],
            "…rebound to the new textures"
        );

        // Hair style: a geoset toggles, so batch 2 goes and batch 3 comes; batch 1 stays.
        bake(
            &mut app,
            create(0, 1, 3, 2),
            HUMAN_MALE,
            vec![part(1, 20), part(3, 22)],
            Vec::new(),
            [false, false],
        );
        assert_eq!(stand_at(&app, root), 1.25, "a hair change keeps the clock");
        let hair = worn(&mut app, root);
        assert_eq!(hair.iter().map(|(n, _, _)| *n).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(hair[0].1, before[0].1, "batch 1 is the same entity");
        assert!(
            app.world().get_entity(before[1].1).is_err(),
            "batch 2's geoset went off, and its entity with it"
        );

        // Class: a new outfit hangs a weapon in the right hand, which closes on it.
        let sword = PreviewRider {
            mesh: handle(30),
            material: handle(31),
            bone: 1,
            offset: Vec3::ZERO,
            twins: BoothTwins::default(),
        };
        bake(
            &mut app,
            create(0, 2, 3, 2),
            HUMAN_MALE,
            vec![part(1, 40), part(3, 41)],
            vec![sword],
            [true, false],
        );
        assert_eq!(stand_at(&app, root), 1.25, "a class change keeps the clock");
        assert_eq!(worn(&mut app, root)[0].1, before[0].1);
        let mut riders = app.world_mut().query::<(&Mesh3d, &ChildOf)>();
        let anchor = riders
            .iter(app.world())
            .find(|(m, _)| m.0.id() == handle::<Mesh>(30).id())
            .map(|(_, c)| c.parent())
            .expect("the weapon hangs");
        assert!(
            app.world()
                .get::<benilla_world::rig_anim::RigAnchor>(anchor)
                .is_some_and(|a| a.rig == root && a.bone == 1),
            "on its bone's anchor of the same body"
        );
        assert!(
            app.world()
                .get::<AnimationPlayer>(root)
                .unwrap()
                .animation(node(GRIP_NODE))
                .is_some(),
            "the hand that holds it closes (`0x479660`)"
        );

        // The create screen shown again: a new scene and a new body, Stand from the top.
        app.world_mut().resource_mut::<GluePreview>().stage += 1;
        bake(
            &mut app,
            create(0, 2, 3, 2),
            HUMAN_MALE,
            vec![part(1, 40), part(3, 41)],
            Vec::new(),
            [false, false],
        );
        assert_eq!(stand_at(&app, root), 0.0, "a re-entered create rebuilds");
        run_stand_to(&mut app, root, 1.25);

        // Sex: the builder makes a new body, whose Stand starts from the top.
        bake(
            &mut app,
            create(1, 2, 0, 0),
            HUMAN_FEMALE,
            vec![part(5, 50)],
            Vec::new(),
            [false, false],
        );
        assert_eq!(stand_at(&app, root), 0.0, "a new body is a new instance");
        assert!(app.world().get_entity(before[0].1).is_err());
    }

    /// A select record is its own model (`record+0x110`), so another record on the same body
    /// display is a new instance.
    #[test]
    fn another_select_record_is_another_model() {
        let (mut app, root) = booth_app();
        let record = |guid| {
            GlueLook::Select(SelectLook::from(
                &crate::glue_session::roster::test_character(guid, "Rec"),
            ))
        };
        bake(
            &mut app,
            record(1),
            HUMAN_MALE,
            vec![part(1, 10)],
            Vec::new(),
            [false, false],
        );
        run_stand_to(&mut app, root, 1.25);
        bake(
            &mut app,
            record(2),
            HUMAN_MALE,
            vec![part(1, 20)],
            Vec::new(),
            [false, false],
        );
        assert_eq!(stand_at(&app, root), 0.0);
    }

    // ── The stage's clock ────────────────────────────────────────────────

    /// A stage of three players on one clock (the scene, the character and the pet), stepped at
    /// a fixed `frame`, with Stand's node.
    struct Stage {
        app: App,
        models: [Entity; 3],
        stand: bevy::animation::graph::AnimationNodeIndex,
    }

    impl Stage {
        fn new(frame: std::time::Duration) -> Self {
            use bevy::animation::{graph::AnimationGraph, AnimationClip};
            let mut app = App::new();
            app.add_plugins(MinimalPlugins)
                .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(frame))
                .init_resource::<Assets<AnimationClip>>()
                .init_resource::<Assets<AnimationGraph>>()
                .init_resource::<GluePreview>()
                .add_systems(
                    Update,
                    (pace_glue_clock, bevy::animation::advance_animations).chain(),
                );
            let mut clip = AnimationClip::default();
            clip.set_duration(100.0);
            let clip = app
                .world_mut()
                .resource_mut::<Assets<AnimationClip>>()
                .add(clip);
            let (graph, stand) = AnimationGraph::from_clip(clip);
            let graph = app
                .world_mut()
                .resource_mut::<Assets<AnimationGraph>>()
                .add(graph);
            let mut model = || {
                let mut player = AnimationPlayer::default();
                player.play(stand).repeat();
                app.world_mut()
                    .spawn((player, AnimationGraphHandle(graph.clone())))
                    .id()
            };
            let models = [model(), model(), model()];
            let [scene_root, character, pet] = models;
            app.insert_resource(CreateScene::empty(scene_root, pet));
            let mut booths = Booths::default();
            booths
                .0
                .insert(GLUE_SLOT.to_string(), glue_booth(character));
            app.insert_resource(booths);
            // The first frame's delta is 0; from the second the stage runs.
            for _ in 0..3 {
                app.update();
            }
            Stage { app, models, stand }
        }

        /// Each model's Stand position, and the scene clock, which the scene's materials read
        /// at the same instant.
        fn at(&self) -> ([f32; 3], u64) {
            let world = self.app.world();
            let clock_ms = world.resource::<CreateScene>().clock_ms;
            for root in self.models {
                assert_eq!(
                    world.get::<benilla_world::rig_anim::SceneClock>(root),
                    Some(&benilla_world::rig_anim::SceneClock(
                        clock_ms as f64 / 1000.0
                    )),
                    "each root on the stage carries the stage clock"
                );
            }
            (
                self.models.map(|e| {
                    world
                        .get::<AnimationPlayer>(e)
                        .and_then(|p| p.animation(self.stand))
                        .unwrap()
                        .seek_time()
                }),
                world.resource::<CreateScene>().clock_ms,
            )
        }

        fn run(&mut self, frames: usize) {
            for _ in 0..frames {
                self.app.update();
            }
        }
    }

    /// A `ModelFFX` advances its scene only in its `OnUpdate` (`0x76d7f0`), which the
    /// visible-frame pump (`0x765650`) calls: hidden, the stage stands still, and shown again it
    /// runs on from where it stopped.
    #[test]
    fn a_hidden_stage_stops_its_clock_and_resumes_where_it_stopped() {
        let mut stage = Stage::new(std::time::Duration::from_millis(100));
        let shown = stage.at();
        assert!(shown.1 > 0 && shown.0.iter().all(|t| *t > 0.0), "{shown:?}");

        stage.app.world_mut().resource_mut::<GluePreview>().hidden = true;
        stage.run(5);
        assert_eq!(stage.at(), shown, "hidden, nothing on the stage advances");
        let parked = |stage: &Stage| {
            let [scene_root, _, pet] = stage.models;
            [scene_root, pet].map(|e| {
                stage
                    .app
                    .world()
                    .get::<benilla_world::rig_anim::AnimParked>(e)
                    .is_some()
            })
        };
        assert_eq!(parked(&stage), [true, true], "and no pose is evaluated");

        stage.app.world_mut().resource_mut::<GluePreview>().hidden = false;
        stage.run(1);
        assert_eq!(parked(&stage), [false, false]);
        let (t, c) = stage.at();
        for (now, then) in t.iter().zip(shown.0) {
            assert!(
                (now - then - 0.1).abs() < 1e-4,
                "shown again, one frame on from the stop: {then} → {now}"
            );
        }
        assert_eq!(c, shown.1 + 100, "{} → {c}", shown.1);
    }

    /// The pane clock's step: each frame's elapsed truncated to whole milliseconds (`0x76d846`),
    /// so a 16.7 ms frame advances the stage 16 ms, and ten of them 160 ms, not 167.
    #[test]
    fn a_stage_steps_in_whole_milliseconds() {
        let mut stage = Stage::new(std::time::Duration::from_micros(16_700));
        let (t0, c0) = stage.at();
        stage.run(10);
        let (t, c) = stage.at();
        assert_eq!(c - c0, 160, "ten 16.7 ms frames step the clock 10 × 16 ms");
        for (now, then) in t.iter().zip(t0) {
            assert!(
                (now - then - 0.160).abs() < 1e-4,
                "and every player steps with it: {then} → {now}"
            );
        }
    }

    // ── A scene left for another pane's ──────────────────────────────────

    /// `AccountLogin.xml`'s `file=` is the login pane's only `SetModel` (`0x76cce0`), so its scene
    /// outlives a visit to the character screens: left for the select stage it is parked (hidden,
    /// posed still, its players and emitters stopped), and the return stands the same root back up
    /// on the clock it stopped at. A scene whose pane let it go is dropped instead.
    #[test]
    fn a_scene_left_for_another_panes_resumes_on_its_return() {
        use bevy::ecs::world::CommandQueue;
        let mut world = World::new();
        let layer = RenderLayers::layer(GLUE_LAYER);
        let login = world
            .spawn((Transform::IDENTITY, Visibility::Visible, layer.clone()))
            .id();
        let pet = world.spawn_empty().id();
        // The login scene: a looping player on its root and one brazier under it.
        let mut player = AnimationPlayer::default();
        player.play(node(STAND_NODE)).repeat();
        world.entity_mut(login).insert(player);
        let brazier = {
            let em = benilla_assets::ModelEmitter {
                def: benilla_world::testing::plain_particle_def(),
                texture: Some(Handle::default()),
                bone_pivot: [0.0; 3],
                billboard: None,
                recursion: None,
                geometry: None,
                owner_reach: 0.0,
                water_bound: (Vec3::ZERO, 0.0),
                idle_seq: 0,
            };
            let mut queue = CommandQueue::default();
            let e = {
                let mut commands = Commands::new(&mut queue, &world);
                benilla_world::particles::spawn_emitter(
                    &mut commands,
                    &em,
                    Transform::IDENTITY,
                    default(),
                    benilla_world::particles::EmitClock::Pinned,
                )
                .expect("a textured emitter spawns")
            };
            queue.apply(&mut world);
            world.entity_mut(e).insert(ChildOf(login));
            e
        };
        let mut scene = CreateScene::empty(login, pet);
        scene.token = Some("MainMenu");
        scene.handle = Some(Handle::default());
        scene.spawned = true;
        scene.stage = 1;
        scene.clock_ms = 1234;

        let step = |world: &mut World, scene: &mut CreateScene, live: &[u64], next: u64| {
            let mut queue = CommandQueue::default();
            let resumed = {
                let children = |e: Entity| {
                    world
                        .get::<Children>(e)
                        .map(|c| c.to_vec())
                        .unwrap_or_default()
                };
                let mut commands = Commands::new(&mut queue, world);
                scene.leave_for(&mut commands, &layer, live, next, &children)
            };
            queue.apply(world);
            resumed
        };
        let frozen = |world: &World| {
            world
                .get::<benilla_world::particles::ParticleEmitter>(brazier)
                .unwrap()
                .is_frozen()
        };
        let speed = |world: &World| {
            world
                .get::<AnimationPlayer>(login)
                .and_then(|p| p.animation(node(STAND_NODE)))
                .unwrap()
                .speed()
        };

        // Login → select: both panes hold their instances, so the login scene parks.
        assert!(!step(&mut world, &mut scene, &[1, 2], 2), "select is new");
        assert_ne!(
            scene.root, login,
            "the select stage spawns on a root of its own"
        );
        assert_eq!(scene.clock_ms, 0, "on a clock of its own, from 0");
        assert_eq!(
            world.get::<benilla_world::rig_anim::SceneClock>(scene.root),
            Some(&benilla_world::rig_anim::SceneClock(0.0)),
            "born on it, so nothing latches the world's clock first"
        );
        assert!(!scene.spawned);
        assert_eq!(*world.get::<Visibility>(login).unwrap(), Visibility::Hidden);
        assert!(world
            .get::<benilla_world::rig_anim::AnimParked>(login)
            .is_some());
        assert_eq!(speed(&world), 0.0, "its clock stands still");
        assert!(frozen(&world), "and its brazier with it");
        assert_eq!(
            world.get::<ChildOf>(brazier).map(|c| c.parent()),
            Some(login),
            "nothing of the scene is torn down"
        );

        // The select stage stands, then the return to login.
        let select = scene.root;
        scene.token = Some("Orc");
        scene.spawned = true;
        scene.stage = 2;
        scene.clock_ms = 50;
        assert!(step(&mut world, &mut scene, &[1, 2], 1), "login resumes");
        assert_eq!(scene.root, login);
        assert_eq!(
            (scene.token, scene.stage, scene.clock_ms, scene.spawned),
            (Some("MainMenu"), 1, 1234, true),
            "on the clock it stopped at"
        );
        assert_eq!(
            *world.get::<Visibility>(login).unwrap(),
            Visibility::Visible
        );
        assert!(world
            .get::<benilla_world::rig_anim::AnimParked>(login)
            .is_none());
        assert!(!frozen(&world));
        assert_eq!(scene.parked.len(), 1, "the select stage parks in turn");

        // The select pane takes a new model: its old scene goes.
        let mut queue = CommandQueue::default();
        scene.evict(&mut Commands::new(&mut queue, &world), &[1]);
        queue.apply(&mut world);
        assert!(scene.parked.is_empty());
        assert!(world.get_entity(select).is_err());
    }

    /// Select stands a race's stage, create stands the same file on its own pane, and Back returns
    /// to select, whose pane still holds its instance (`currentModel` unchanged, no `SetModel`).
    /// Each scene stands on a root of its own, which keys its own materials
    /// (`M2BatchMaterials::off_world_for`), so the resumed select scene's rows run on its clock
    /// and not on the parked create scene's; a root that goes hands its materials back.
    #[test]
    fn select_create_back_keeps_each_scene_on_its_own_root_and_clock() {
        use bevy::ecs::world::CommandQueue;
        let mut world = World::new();
        let layer = RenderLayers::layer(GLUE_LAYER);
        let first = world.spawn(Transform::IDENTITY).id();
        let pet = world.spawn_empty().id();
        let mut scene = CreateScene::empty(first, pet);
        let step = |world: &mut World, scene: &mut CreateScene, live: &[u64], next: u64| {
            let mut queue = CommandQueue::default();
            let resumed = {
                let mut commands = Commands::new(&mut queue, world);
                scene.leave_for(&mut commands, &layer, live, next, &|_| Vec::new())
            };
            queue.apply(world);
            resumed
        };
        let stand = |scene: &mut CreateScene, stage: u64, clock_ms: u64| {
            scene.token = Some("Orc");
            scene.handle = Some(Handle::default());
            scene.spawned = true;
            scene.stage = stage;
            scene.clock_ms = clock_ms;
        };

        // The first stage replaces the boot root, which held nothing of a scene.
        assert!(!step(&mut world, &mut scene, &[2], 2));
        assert_eq!(scene.dropped, vec![first], "the boot root goes");
        scene.dropped.clear();
        let select = scene.root;
        stand(&mut scene, 2, 700);

        // Create: select's pane still holds UI_Orc, so it parks; create stands UI_Orc anew.
        assert!(!step(&mut world, &mut scene, &[2, 3], 3));
        let create = scene.root;
        assert_ne!(create, select, "the same file, two scenes, two roots");
        assert!(scene.dropped.is_empty(), "nothing went");
        stand(&mut scene, 3, 0);
        scene.clock_ms = 400;

        // Back: select resumes on its own root and its own clock; create parks.
        assert!(step(&mut world, &mut scene, &[2, 3], 2));
        assert_eq!((scene.root, scene.clock_ms), (select, 700));
        assert_eq!(
            scene.parked.iter().map(|p| p.root).collect::<Vec<_>>(),
            [create]
        );

        // The next create show makes a new instance: the old create scene goes, with its
        // materials.
        let mut queue = CommandQueue::default();
        scene.evict(&mut Commands::new(&mut queue, &world), &[2]);
        queue.apply(&mut world);
        assert_eq!(scene.dropped, vec![create]);
        assert!(world.get_entity(create).is_err());
    }

    /// Every stage root is born on a scene clock at 0, so a reader running before the first pace
    /// (an alpha's global-sequence anchor) never latches the world's clock.
    #[test]
    fn the_glue_booths_roots_are_born_on_a_zero_scene_clock() {
        use bevy::ecs::world::CommandQueue;
        let mut world = World::new();
        let mut images = Assets::<Image>::default();
        let mut portraits = PortraitImages::default();
        let mut booths = Booths::default();
        let mut queue = CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            spawn_glue_booth(&mut commands, &mut images, &mut portraits, &mut booths);
        }
        queue.apply(&mut world);
        let scene = world.resource::<CreateScene>();
        let roots = [booths.0[GLUE_SLOT].root, scene.root, scene.pet_root];
        for root in roots {
            assert_eq!(
                world.get::<benilla_world::rig_anim::SceneClock>(root),
                Some(&benilla_world::rig_anim::SceneClock(0.0))
            );
        }
    }

    /// Off the glue screens every scene goes, parked or standing, and so do the relit twins of the
    /// models that stood, so nothing holds their materials and the tick frees their rows.
    #[test]
    fn standing_down_drops_every_scene_and_every_twin() {
        use bevy::ecs::world::CommandQueue;
        let mut world = World::new();
        let standing = world.spawn(Transform::IDENTITY).id();
        let parked_root = world.spawn(Transform::IDENTITY).id();
        let pet = world.spawn_empty().id();
        let mut scene = CreateScene::empty(standing, pet);
        scene.token = Some("Orc");
        scene.handle = Some(Handle::default());
        scene.spawned = true;
        scene.parked.push(ParkedScene {
            root: parked_root,
            token: "MainMenu",
            handle: Handle::default(),
            cam: None,
            char_spot: Vec3::ZERO,
            char_facing: Quat::IDENTITY,
            char_scale: 1.0,
            pet_spot: Vec3::ZERO,
            pet_facing: Quat::IDENTITY,
            pet_stage_scale: 1.0,
            stage: 1,
            clock_ms: 0,
        });
        scene
            .variants
            .insert(handle::<WowModelMaterial>(1).id(), handle(2));
        let mut queue = CommandQueue::default();
        let stood = scene.stand_down(&mut Commands::new(&mut queue, &world));
        queue.apply(&mut world);
        assert!(stood);
        assert!(scene.variants.is_empty(), "the twins go");
        assert!(scene.parked.is_empty() && scene.token.is_none());
        assert_eq!(scene.dropped, vec![parked_root, standing]);
        assert!(world.get_entity(parked_root).is_err());
    }
}
