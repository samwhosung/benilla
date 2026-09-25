//! The visuals of streamed entities ([`crate::net::NetEntity`]), attached once the model loads: a
//! unit's display resolves to its M2 through the creature chain, a player's body included, a
//! GameObject's to its model, and anything else draws as a cube.

use std::collections::HashMap;

use benilla_assets::{M2Model, WmoModel};
use benilla_formats::{
    load_creature_catalog, load_gameobject_catalog, load_item_display_catalog, CharCreateCatalog,
    CharSections, CharacterGeosets, CreatureCatalog, GameObjectCatalog,
};
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::NetEntity;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::model_fade::apply_render_fade;
use benilla_world::schedule::WorldStage;

/// The per-display model cache: a display id's [`DisplayModel`] and its spawn parts.
pub(crate) mod display;
use display::{
    build_parts, empty_display, empty_shell, new_creature_display, new_gameobject_display,
    DisplayModel, EntityPart, ModelHandle,
};

/// Attaching a visual to each net entity: skeleton, animation, character geosets and skin, fade.
mod attach;
use attach::{attach_entity_visuals, build_dressup_preview, build_glue_pet, build_glue_preview};

/// The dynamic point lights an entity's own model carries, such as a held torch.
mod carried_light;
use carried_light::spawn_carried_lights;

/// Equipment from the descriptor and `ItemDisplayInfo`: armour, held items, helm and shoulders.
mod equipment;
use equipment::{attach_held_items, resolve_corpse_equipment, resolve_equipment};

/// Item and enchant glows, hung on the item's own attachment points.
mod item_glow;
pub(crate) use item_glow::ItemGlowAttached;
use item_glow::{attach_item_glows, ItemGlows};

/// Mounts: a second creature visual the rider's model is re-parented onto, never rebuilt.
pub(crate) mod mount;
use mount::reseat_mounts;

/// The corpse object, drawn as the dressed body or, once it turns to bones, a skeleton prop.
pub(crate) mod corpse;

mod live_display;
pub(crate) use live_display::DisplaySwapped;
use live_display::{refresh_live_display, tick_scale_ease};

/// Terrain conform: a model flagged `GlobalModelFlags & 3 ∈ {1,3}` tilts to the ground.
mod conform;

/// The per-unit collision height, the `h` the swim, wade, splash and foam depths are fractions of.
mod collision_height;
use collision_height::stamp_collision_heights;
pub(crate) use collision_height::CollisionHeight;

/// Spell missiles: the projectile a cast with a `Spell.dbc` Speed above 0 flies at each target.
mod missile;
use missile::{attach_missile_models, move_missiles, spawn_missiles};
pub(crate) use missile::{MissileMiss, MissileSound, PendingMissiles};

/// A WMO-display GameObject's doodad props (a ship's sails), parented under it to ride a transport.
mod wmo_props;
use wmo_props::{resolve_wmo_gameobject_props, spawn_wmo_gameobject_props};
pub(crate) mod spell_fx;
use spell_fx::{attach_spell_fx, resolve_spell_fx};

/// Dest-anchored spell effects: a DynamicObject's area visual and a cast's burst at its point.
pub(crate) mod dest_fx;
use dest_fx::{
    arm_ground_effects, attach_ground_fx_models, spawn_ground_bursts, tick_shard_emitters,
};

/// Spell chain beams, such as Chain Lightning's arcs.
mod chain_beam;
pub(crate) use chain_beam::ChainHops;
use chain_beam::{simulate_chain_beams, spawn_chain_beams};
// `equip_slot` is the one InventoryType → slot table; the dressing room places items by it.
pub(crate) use attach::{equip_slot, BodyPartsDesc};
pub(crate) use equipment::ItemDisplays;
pub(crate) use equipment::{BoneAttach, Equipment};
// For the `WOW_DRESS_CENSUS` instrument: what a body wears against what it resolved.
pub(crate) use equipment::{attach_id, DressKey, HeldAttached, ATTACH_SLOT_NAMES};
// Only the tests that build a `DressKey` name this type.
#[cfg(test)]
pub(crate) use equipment::ItemModelKind;

/// The overhead attachment (`PlayerName`, id 18), where the name, combat text and quest marker sit.
pub(crate) const ATTACH_OVERHEAD: u16 = 18;

/// The mounted overhead attachment (`PlayerNameMounted`, id 29), on the rider's own model,
/// preferred while a mount model is attached (`0x6074c0`, `0x608640`).
pub(crate) const ATTACH_OVERHEAD_MOUNTED: u16 = 29;

/// The overhead slot for a unit now, the marker attach's pick on the body model (`0x6074c0`): 29
/// when a mount model exists (`unit+0xdc`) and the body authors it, else 18, else `None` (never
/// parented). The mount handler re-picks on every mount and dismount (`0x5ffa50` at `0x5ffae7`),
/// so a consumer parented at the slot must re-parent.
pub(crate) fn overhead_slot(attach: &BoneAttach, mounted: bool) -> Option<u16> {
    if mounted && attach.points.contains_key(&ATTACH_OVERHEAD_MOUNTED) {
        Some(ATTACH_OVERHEAD_MOUNTED)
    } else if attach.points.contains_key(&ATTACH_OVERHEAD) {
        Some(ATTACH_OVERHEAD)
    } else {
        None
    }
}

/// The `0x608640` fallback multiplier (`0x80c5d0` = 1.25): a unit whose model has no PlayerName
/// attachment anchors overhead content at `feet + scale × bbox_z × 1.25`.
const OVERHEAD_FALLBACK_FACTOR: f32 = 1.25;

/// The Stand sequence's box height (model-local, pre-scale), the chat bubble's anchor alone. The
/// reference reads it from the model file's header with no pose (`0x711a20`), once per chat line
/// (cached at bubble `+0x354`), while the name's anchor is the posed attachment (`0x608640`).
#[derive(Component)]
pub(crate) struct StandBoxHeight(pub(crate) f32);

/// The model's bbox z-extent (model-local, pre-scale) for [`overhead_anchor`]'s fallback; `0.0`
/// until the model loads, which anchors at the feet as the client does with no model.
#[derive(Component)]
pub(crate) struct OverheadFallback(pub(crate) f32);

/// The client's per-attachment z-bias table (`0x862708`, ids `0..=0x24`): how far above the unit's
/// position a sound plays when its model lacks the attachment asked for.
const ATTACH_Z_BIAS: [f32; 37] = [
    1.0, 1.0, 1.0, 1.5, 1.5, 1.8, 1.8, 0.5, 0.5, 1.0, 1.0, 2.0, 1.5, 1.0, 1.0, 1.0, 1.0, 2.0, 2.5,
    0.0, 2.0, 1.5, 1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 3.0, 1.5, 1.5, 1.0, 1.0, 1.5, 1.0, 1.0,
];

/// Where a unit-attached sound plays (`0x623b90`): the attachment's live world point when the
/// model carries it (`0x712cb0`, `0x712d50`), else the unit's position raised by [`ATTACH_Z_BIAS`]
/// (`0x623be2`). The emote voice `$CSD` asks for 17 (`0x623c3a`) and a whiffed swing's `$CSS` for
/// 1 (`0x624bdd`), never the fired event's own point.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AttachPoints<'w, 's> {
    anchors: Query<'w, 's, &'static BoneAttach>,
    poses: Query<'w, 's, &'static benilla_world::rig_anim::RigPose>,
    globals: Query<'w, 's, &'static GlobalTransform>,
}

impl AttachPoints<'_, '_> {
    /// The world point; `fallback` is the unit's position, the reference's `GetPosition`.
    pub(crate) fn point(&self, entity: Entity, attach: u16, fallback: Vec3) -> Vec3 {
        self.anchors
            .get(entity)
            .ok()
            .and_then(|a| {
                let &(bone, offset) = a.points.get(&attach)?;
                let pose = self.poses.get(entity).ok()?;
                pose.posed_point(self.globals.get(pose.joints_root).ok()?, bone, offset)
            })
            .unwrap_or_else(|| {
                fallback + Vec3::Y * ATTACH_Z_BIAS.get(attach as usize).copied().unwrap_or(0.0)
            })
    }
}

/// The unit's overhead anchor in world space (`0x608640`): the posed PlayerName attachment (29
/// while a mount model is attached, else 18), else `feet + scale × bbox_z × 1.25`.
///
/// When the rig root is the unit itself its frame comes from `tf`, not the `GlobalTransform` Bevy
/// propagates in `PostUpdate`, which an `Update` reader sees a frame late (the chat bubble slid
/// against the head). That is exact because a streamed unit is a world-root entity; a mounted
/// rider or a conform-tilted model, rooted elsewhere, still reads the late propagated frame.
pub(crate) fn overhead_anchor<F: bevy::ecs::query::QueryFilter>(
    entity: Entity,
    tf: &Transform,
    anchors: &Query<&BoneAttach>,
    poses: &Query<&benilla_world::rig_anim::RigPose>,
    fallbacks: &Query<&OverheadFallback>,
    globals: &Query<&GlobalTransform, F>,
    mounts: &Query<(), With<mount::MountChild>>,
) -> Vec3 {
    anchors
        .get(entity)
        .ok()
        .and_then(|a| {
            let &(bone, offset) = a.points.get(&overhead_slot(a, mounts.contains(entity))?)?;
            let pose = poses.get(entity).ok()?;
            let own;
            let root = if pose.joints_root == entity {
                own = GlobalTransform::from(*tf);
                &own
            } else {
                globals.get(pose.joints_root).ok()?
            };
            pose.posed_point(root, bone, offset)
        })
        .unwrap_or_else(|| {
            let bbox_z = fallbacks.get(entity).map_or(0.0, |f| f.0);
            tf.translation + Vec3::Y * (tf.scale.y * bbox_z * OVERHEAD_FALLBACK_FACTOR)
        })
}

/// Marks the cube spawned for an entity whose display named no loadable model, so
/// `WOW_UNIT_VISUALS` can count the cubes and their displays.
#[derive(Component)]
pub(crate) struct FallbackCube;

/// The fallback cube mesh and per-kind materials. There is no GameObject cube: a model-less
/// GameObject is an invisible effect or trigger in the reference.
#[derive(Resource)]
pub(crate) struct CubeAssets {
    mesh: Handle<Mesh>,
    /// The slimmer block for a player whose body model is unavailable.
    player_mesh: Handle<Mesh>,
    player_mat: Handle<StandardMaterial>,
    npc_mat: Handle<StandardMaterial>,
}

impl CubeAssets {
    /// The cube mesh and materials, for [`crate::pipe_warm`] to compile behind the loading cover.
    pub(crate) fn warm_parts(&self) -> (Handle<Mesh>, [Handle<StandardMaterial>; 2]) {
        (
            self.mesh.clone(),
            [self.player_mat.clone(), self.npc_mat.clone()],
        )
    }
}

/// The creature display catalog and per-display [`DisplayModel`] cache; absent, NPCs are cubes.
#[derive(Resource)]
pub(crate) struct Creatures {
    catalog: CreatureCatalog,
    models: HashMap<u32, DisplayModel>,
}

impl Creatures {
    /// A display's foley material (`Material.dbc` id), the creature half of the footfall rustle.
    pub(crate) fn foley_material(&self, display_id: u32) -> Option<u32> {
        self.catalog.foley_material(display_id)
    }

    /// A display's collision height in raw model units; [`CollisionHeight`] is the world value.
    pub(crate) fn collision_height(&self, display_id: u32) -> Option<f32> {
        self.catalog.collision_height(display_id)
    }

    /// A display's footprint-decal parameters (yards, pre-scale); `None` leaves no prints.
    pub(crate) fn footprint(&self, display_id: u32) -> Option<benilla_formats::FootprintParams> {
        self.catalog.footprint(display_id)
    }

    /// Whether the model wears `$BTH` breath puffs, which `CreatureModelData.Flags & 0x2` stops.
    pub(crate) fn breathes(&self, display_id: u32) -> bool {
        self.catalog.breathes(display_id)
    }

    /// A display's `CreatureModelAlpha / 255`, the aura layer's `baseAlpha`.
    pub(crate) fn display_base_alpha(&self, display_id: u32) -> Option<f32> {
        self.catalog.display_base_alpha(display_id)
    }

    /// A display's footstep camera shake (`FootstepShakeSize`); `None` for 405 of 430 models.
    pub(crate) fn footstep_shake(&self, display_id: u32) -> Option<u32> {
        self.catalog.footstep_shake(display_id)
    }

    /// A display's death-thud camera shake (`DeathThudShakeSize`), fired on `$DTH`.
    pub(crate) fn death_thud_shake(&self, display_id: u32) -> Option<u32> {
        self.catalog.death_thud_shake(display_id)
    }

    /// A display's audible size class (0 Small to 4 Colossal), which picks the body-fall sample.
    pub(crate) fn size_class(&self, display_id: u32) -> Option<u32> {
        self.catalog.size_class(display_id)
    }

    /// A display's `modelScale × creatureModelScale`; only the glue screens' pet reads it, as the
    /// server folds it into a spawned unit's `OBJECT_FIELD_SCALE_X`.
    pub(crate) fn model_scale(&self, display_id: u32) -> Option<f32> {
        self.catalog.model_scale(display_id)
    }

    /// `CreatureDisplayInfo.BloodLevel` and `CreatureModelData.BloodID`, the reference's blood
    /// tiers 1 and 2; [`benilla_formats::BloodCatalog::level_key`] adds tier 3.
    pub(crate) fn blood_candidates(&self, display_id: u32) -> Option<(i32, i32)> {
        self.catalog
            .model(display_id)
            .map(|m| (m.blood_display, m.blood_model))
    }

    /// A built display's booth framing (model-local, pre-scale): its authored cameras, index 0 of
    /// `cameraLookup` for the round portrait (`0x713540`) and raw index 1 for a `<PlayerModel>`
    /// pane (`0x505890`), with each path's fallback for a model that has none.
    pub(crate) fn display_anchors(
        &self,
        display_id: u32,
    ) -> Option<crate::portrait::PortraitAnchors> {
        let dm = self.models.get(&display_id)?;
        dm.parts.as_ref()?; // not yet built
        Some(crate::portrait::PortraitAnchors {
            camera: dm.portrait_camera,
            pane_camera: dm.pane_camera,
            bbox_center: dm.bake_center_local,
            head: crate::portrait::head_anchor(&dm.skeleton, &dm.attachments),
            pivot_height: dm.pivot_height_local,
            ground_radius: dm.ground_radius_local,
        })
    }

    /// A built display's `(PortraitPart, PortraitBillboard)` lists, straight from the cache, for a
    /// creature with no world entity ([`crate::portrait::PortraitStandIn`]): every batch, the
    /// camera-facing ones as cards, since only the character compositor selects geosets.
    pub(crate) fn display_mirror(
        &self,
        display_id: u32,
    ) -> Option<(
        Vec<crate::portrait::PortraitPart>,
        Vec<crate::portrait::PortraitBillboard>,
    )> {
        let parts = self.models.get(&display_id)?.parts.as_deref()?;
        let mut meshes = Vec::new();
        let mut cards = Vec::new();
        for part in parts {
            match &part.billboard {
                Some(info) => cards.push(crate::portrait::PortraitBillboard {
                    mesh: part.mesh.clone(),
                    material: part.material.clone(),
                    bone: info.bone,
                    seat: crate::portrait::PortraitSeat::Body,
                    kind: info.kind,
                    attach: None,
                }),
                None => meshes.push(crate::portrait::PortraitPart {
                    static_mesh: part.mesh.clone(),
                    skinned_mesh: part.skinned_mesh.clone(),
                    material: part.material.clone(),
                }),
            }
        }
        Some((meshes, cards))
    }

    /// A built display's rig, for the booth to pose a fresh instance at Stand as the reference's
    /// bake does (`0x524f60`), not the unit's live pose; a WMO display's skeleton is empty.
    pub(crate) fn display_rig(&self, display_id: u32) -> Option<DisplayRig<'_>> {
        let dm = self.models.get(&display_id)?;
        dm.parts.as_ref()?; // not yet built
        Some(DisplayRig {
            skeleton: &dm.skeleton,
            inverse_bindposes: dm.inverse_bindposes.clone(),
            animations: dm.animations.as_ref(),
        })
    }
}

/// The skeleton/animation surface the portrait booth poses a bake with ([`Creatures::display_rig`]).
pub(crate) struct DisplayRig<'a> {
    pub(crate) skeleton: &'a benilla_assets::ModelSkeleton,
    /// `None` only for a WMO or model-less display, whose skeleton is empty.
    pub(crate) inverse_bindposes: Option<Handle<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    pub(crate) animations: Option<&'a benilla_assets::ModelAnimations>,
}

/// The `GameObjectDisplayInfo` catalog and per-display [`DisplayModel`] cache.
#[derive(Resource)]
struct GameObjects {
    catalog: GameObjectCatalog,
    models: HashMap<u32, DisplayModel>,
}

/// The customization → visible-geoset tables; without them players render every geoset.
#[derive(Resource)]
struct Characters(CharacterGeosets);

/// The `CharSections` skin lookup; without it a player's body skin stays untextured.
#[derive(Resource)]
struct SkinSections(CharSections);

/// Character-creation data (body displays, race and class combos, appearance ranges).
#[derive(Resource)]
pub(crate) struct CharCreate(pub(crate) CharCreateCatalog);

/// Composited body skins by look, so every player wearing a look shares one 256² atlas.
#[derive(Resource, Default)]
struct SkinComposites(benilla_assets::SpatialCache<SkinKey, Handle<Image>>);

/// What decides a composited body skin: race and sex pick the `CharSections` rows, the dials pick
/// the variations, and `equip` holds the worn armour display ids by body slot − 2.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SkinKey {
    pub(super) race: u8,
    pub(super) sex: u8,
    pub(super) skin: u8,
    pub(super) face: u8,
    pub(super) facial_hair: u8,
    pub(super) hair_style: u8,
    pub(super) hair_color: u8,
    pub(super) equip: [u32; 8],
    /// The guild emblem: two guilds' members wear one tabard display but must not share an atlas.
    pub(super) emblem: Option<benilla_formats::GuildEmblem>,
    /// The tabard designer's preview: the emblem paints over an empty tabard slot.
    pub(super) tabard_preview: bool,
}

/// Marks a net entity whose visual is attached; the `waterfx` rig pre-marks its dummy unit.
#[derive(Component)]
pub(crate) struct VisualAttached;

/// The per-frame entity-visuals chain as one set; [`crate::creature_anim::VisualSheath`] must land
/// before it, or a sheath change places the weapon twice in one frame.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct EntityVisualsSet;

/// [`update_display_models`]' set: a creator of display-cache entries orders before it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DisplayBuildSet;

/// Drop every display and skin cache on a map change; an entry rebuilds on its next use.
fn evict_display_caches(
    mut changes: MessageReader<benilla_world::world_map::MapChange>,
    mut composites: ResMut<SkinComposites>,
    mut fx: ResMut<spell_fx::SpellFx>,
    creatures: Option<ResMut<Creatures>>,
    gos: Option<ResMut<GameObjects>>,
    items: Option<ResMut<equipment::ItemDisplays>>,
    glows: Option<ResMut<ItemGlows>>,
    mut bones: ResMut<corpse::BonesModels>,
) {
    if changes.is_empty() {
        return;
    }
    changes.clear();
    info!(
        "display caches evicted: {} creature / {} go / {} item / {} fx / {} glow models, {} skins",
        creatures.as_ref().map_or(0, |c| c.models.len()),
        gos.as_ref().map_or(0, |g| g.models.len()),
        items.as_ref().map_or(0, |i| i.models.len()),
        fx.models.len(),
        glows.as_ref().map_or(0, |g| g.models.len()),
        composites.0.len(),
    );
    composites.0.clear();
    fx.models.clear();
    if let Some(mut c) = creatures {
        c.models.clear();
    }
    if let Some(mut g) = gos {
        g.models.clear();
    }
    if let Some(mut i) = items {
        i.models.clear();
    }
    if let Some(mut g) = glows {
        g.models.clear();
    }
    bones.0.clear();
}

/// Expire composited skins by distance within a map. The display-id model caches are not swept:
/// they key on ids, not places, so the catalogs bound them.
fn scope_entity_art(
    mut scope: benilla_world::art_scope::ArtScope,
    mut composites: ResMut<SkinComposites>,
) {
    scope.apply(&mut composites.0, benilla_world::art_scope::ArtSlot::Skins);
}

/// A built body's armed-idle box in model space, which [`publish_world_units`] restates as
/// `WorldUnit::bound`; absent until the model resolves.
#[derive(Component, Clone, Copy)]
pub(crate) struct ModelBound(pub(crate) bevy::camera::primitives::Aabb);

/// What [`publish_world_units`] reads to restate one body.
type WireBody = (
    Entity,
    &'static NetEntity,
    Option<&'static collision_height::CollisionHeight>,
    Option<&'static ModelBound>,
    Has<crate::transport::TransportAnchor>,
    Option<&'static benilla_world::world_unit::WorldUnit>,
);

type ViewerRow = (
    Entity,
    Has<crate::net::Embodied>,
    Has<benilla_world::world_unit::ViewerUnit>,
);

type ViewerCandidate = Or<(
    With<crate::net::Embodied>,
    With<benilla_world::world_unit::ViewerUnit>,
)>;

/// The edges that can move a body's restatement; a removed transport anchor is read separately.
type WireBodyMoved = Or<(
    Changed<NetEntity>,
    Changed<collision_height::CollisionHeight>,
    Changed<ModelBound>,
    Added<crate::transport::TransportAnchor>,
)>;

/// Restate every wire body as a [`WorldUnit`](benilla_world::world_unit::WorldUnit), in one
/// reconciler rather than at each spawn site. Runs right after the wire drain, so a unit that
/// arrived this frame is visible to the world this frame.
fn publish_world_units(
    mut commands: Commands,
    // Only bodies whose inputs moved; leaving a transport removes the anchor with no `Changed`,
    // so that edge comes from `unanchored`.
    bodies: Query<WireBody, WireBodyMoved>,
    all_bodies: Query<WireBody>,
    mut unanchored: RemovedComponents<crate::transport::TransportAnchor>,
    viewers: Query<ViewerRow, ViewerCandidate>,
) {
    let freed: Vec<Entity> = unanchored.read().collect();
    let due = bodies
        .iter()
        .chain(freed.iter().filter_map(|&e| all_bodies.get(e).ok()));
    for (entity, net, height, bound, anchored, current) in due {
        let want = benilla_world::world_unit::WorldUnit {
            // A live body wades; a GameObject or dynamic object standing in a lake makes no ripple.
            wades: matches!(net.kind, EntityKind::Unit | EntityKind::Player),
            scale: net.scale,
            // The client's constructor default until the display resolves, never 0.0, at which
            // every depth line collapses.
            height: height.copied().unwrap_or_default().0,
            // The box the exterior cull elects this body by. A transport answers `None`, as
            // `transport::tick_transports` alone writes its `Visibility`. Every other body is
            // elected from its first frame, on a point box at its origin until the model
            // resolves, so a streaming mob is never drawn through a sealed ceiling.
            bound: (!anchored).then(|| {
                bound.map_or_else(
                    || bevy::camera::primitives::Aabb::from_min_max(Vec3::ZERO, Vec3::ZERO),
                    |b| b.0,
                )
            }),
        };
        // Written only on a real change: its readers are change-detected.
        let same = current.is_some_and(|c| {
            c.wades == want.wades
                && c.scale == want.scale
                && c.height == want.height
                && c.bound == want.bound
        });
        if !same {
            commands.entity(entity).insert(want);
        }
    }
    // Reconciled apart from `NetEntity`: the self entity exists before its wire record and
    // outlives it on `/logout`.
    for (entity, is_self, marked) in &viewers {
        match (is_self, marked) {
            (true, false) => {
                commands
                    .entity(entity)
                    .insert(benilla_world::world_unit::ViewerUnit);
            }
            (false, true) => {
                commands
                    .entity(entity)
                    .remove::<benilla_world::world_unit::ViewerUnit>();
            }
            _ => {}
        }
    }
}

/// Loads the display catalogs at startup and each frame gives every net entity its visual.
pub(crate) struct EntitiesPlugin;

impl Plugin for EntitiesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            publish_world_units
                .after(benilla_world::schedule::WorldStage::Net)
                .before(benilla_world::schedule::WorldStage::Input),
        )
        .init_resource::<SkinComposites>()
        .init_resource::<attach::MergedFormsCache>()
        // The 16 bone-pile bodies, keyed by (race, sex): a skeleton has no display row.
        .init_resource::<corpse::BonesModels>()
        .init_resource::<spell_fx::SpellFx>()
        .init_resource::<spell_fx::FxTintAnims>()
        // The per-caster pending-projectile queues (the client's `unit+0xac` lists).
        .init_resource::<missile::PendingMissiles>()
        .add_message::<MissileSound>()
        .add_message::<MissileMiss>()
        .add_message::<dest_fx::GroundBurst>()
        // A display swap's rebuild edge, for the impact-kit replay the reference runs at the end
        // of `0x60abe0`.
        .add_message::<live_display::DisplaySwapped>()
        .add_systems(Startup, setup_entities.after(AssetSet::Open))
        .add_systems(Update, (evict_display_caches, scope_entity_art))
        // After the net stage, whose Commands create the entity; until then its readers take the
        // constructor default.
        .add_systems(Update, stamp_collision_heights.after(WorldStage::Net))
        // Display models resolve and build before attach, all after `WorldStage::Net` spawns the
        // entities, or attach would meet an unresolved display and lock in a cube.
        .add_systems(
            Update,
            (
                // Equipment resolution creates the item display entries built the same frame.
                // The nested tuples are unordered groups: the outer tuple is at `chain()`'s
                // 20-element ceiling.
                (resolve_equipment, resolve_corpse_equipment),
                // Spell effects and missiles create `SpellFx` entries too.
                resolve_spell_fx,
                move_missiles,
                spawn_missiles,
                // The dest-anchored lane's three independent producers also create entries.
                (arm_ground_effects, spawn_ground_bursts, tick_shard_emitters),
                update_display_models.in_set(DisplayBuildSet),
                // The glue screens' character, assembled from the display built just above.
                build_glue_preview,
                // The select screen's pet, on its own latch so a slow pet model never holds the
                // character back.
                build_glue_pet,
                // The dressing room's preview, the same assembly and latch.
                build_dressup_preview,
                attach_entity_visuals,
                // A WMO GameObject's doodad props, spawned as each M2 lands.
                resolve_wmo_gameobject_props,
                spawn_wmo_gameobject_props,
                attach_held_items,
                // Spell-kit effect models and held-item glows, after the item roots exist.
                (attach_item_glows, attach_spell_fx),
                (
                    attach_missile_models,
                    attach_ground_fx_models,
                    spell_fx::tend_world_plants,
                ),
                // After attach, so a tint clone's first drawn frame is on its own clock.
                spell_fx::tick_fx_tint,
                // Event keyframes and completion callbacks, after attach so a just-spawned
                // instance's first window `[0, cur]` fires this frame.
                (spell_fx::fire_fx_anim_events, spell_fx::advance_fx_anim),
                // A gear change re-dresses the standing visual in place, every attachment left
                // alone as in the reference; a new corpse arms its Dead or Drowned pose.
                (attach::redress_player_looks, corpse::pose_corpses),
                // A mount change re-seats the rig on the mount's attachment 0 or back, as the
                // reference re-parents the body model (`0x712f70`, `0x713020`).
                reseat_mounts,
                // A display swap rebuilds, a scale change eases over the reference's 2 s, and a
                // unit denied a palette rig rebuilds once the table has room.
                (
                    refresh_live_display,
                    tick_scale_ease,
                    live_display::heal_rig_starved,
                )
                    .chain(),
            )
                .chain()
                .in_set(EntityVisualsSet)
                .after(WorldStage::Net),
        )
        // In the visuals set, after the cast router's beam plays and the net stage's hop arrays.
        .add_systems(
            Update,
            spawn_chain_beams
                .in_set(EntityVisualsSet)
                .after(WorldStage::Net),
        )
        // The beam's endpoints are attachment joints, sampled from the pose the billboard
        // palette and the rig finalizer just wrote. `.after(begin_effect_frame)` is load-bearing:
        // without it the shared effect stream's clear can run after the sim and wipe the beam.
        .add_systems(
            PostUpdate,
            simulate_chain_beams
                .in_set(benilla_world::billboard::BillboardPlace)
                .after(benilla_world::billboard::billboard_joint_palette)
                .after(benilla_world::rig_anim::finalize_rig_worlds)
                .after(benilla_world::particles::buffer::begin_effect_frame),
        )
        // Terrain conform (`0x614cd0` → `0x7106c0`), before propagation so the globals carry this
        // frame's tilt.
        .add_systems(
            PostUpdate,
            conform::conform_units.before(bevy::transform::TransformSystems::Propagate),
        )
        // After every steady-state writer of the render-alpha field, before the self-avatar
        // feather that overrides it.
        .add_systems(
            Update,
            apply_unit_mat_alpha
                .after(benilla_world::interior::classify_entity_interior)
                .after(benilla_world::model_render::ModelVisSet)
                .after(apply_render_fade)
                .before(crate::player::apply_self_model_fade),
        )
        // The aura CharProc layer (`crate::aura_visual`), after the steady-state alpha writers so
        // its override lands, before the self feather, which folds the aura factor in itself.
        .add_systems(
            Update,
            (
                // The display's base alpha first (`0x604990`'s leg of `0x60d180`), so a same-frame
                // aura edge retargets from the new base.
                crate::aura_visual::refresh_base_alpha,
                crate::aura_visual::drain_aura_procs,
                // The tint publish needs only the drain ahead of it.
                (
                    crate::aura_visual::apply_aura_alpha,
                    crate::aura_visual::apply_aura_tint,
                ),
            )
                .chain()
                .after(apply_unit_mat_alpha)
                .before(crate::player::apply_self_model_fade),
        );
    }
}

/// Build the cube assets and load the display and data catalogs; a failed load leaves its
/// resource absent, which every reader treats as optional.
fn setup_entities(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world_assets: Option<ResMut<WorldAssets>>,
) {
    // A person-sized box; its origin is centred, so it is lifted by half its height.
    let mesh = meshes.add(Cuboid::new(2.0, 4.0, 2.0));
    let player_mesh = meshes.add(Cuboid::new(0.8, 2.0, 0.8));
    let mut mat = |r, g, b| {
        materials.add(StandardMaterial {
            base_color: Color::linear_rgb(r, g, b), // raw into the gamma lane
            perceptual_roughness: 0.7,
            ..default()
        })
    };
    commands.insert_resource(CubeAssets {
        mesh,
        player_mesh,
        player_mat: mat(0.1, 0.85, 0.9), // cyan: players
        npc_mat: mat(0.9, 0.25, 0.2),    // red: NPCs
    });

    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match load_creature_catalog(&mut chain) {
        Ok(catalog) => {
            info!(
                "creature catalog: {} display entries, {} character-model NPC appearances",
                catalog.len(),
                catalog.extra_len()
            );
            commands.insert_resource(Creatures {
                catalog,
                models: HashMap::new(),
            });
        }
        Err(e) => warn!("creature catalog unavailable, NPCs stay cubes: {e:#}"),
    }
    match load_gameobject_catalog(&mut chain) {
        Ok(catalog) => {
            info!("gameobject catalog: {} display entries", catalog.len());
            commands.insert_resource(GameObjects {
                catalog,
                models: HashMap::new(),
            });
        }
        Err(e) => warn!("gameobject catalog unavailable, GameObjects stay cubes: {e:#}"),
    }
    match benilla_formats::load_lock_catalog(&mut chain) {
        Ok(catalog) => {
            info!("lock catalog: {} locks", catalog.len());
            commands.insert_resource(crate::go_templates::Locks(catalog));
        }
        // Without lock data every GameObject reads as lockless: a right-click sends USE.
        Err(e) => warn!("lock catalog unavailable, GameObjects treated as lockless: {e:#}"),
    }
    match benilla_formats::load_lock_type_catalog(&mut chain) {
        Ok(catalog) => {
            info!(
                "lock-type catalog: {} cursor-bearing lock kinds",
                catalog.len()
            );
            commands.insert_resource(crate::go_templates::LockTypes(catalog));
        }
        // Without `LockType` data a locked GameObject shows the Interact cursor.
        Err(e) => warn!("lock-type catalog unavailable, GO cursors fall back to Interact: {e:#}"),
    }
    match (
        benilla_formats::load_pet_personalities(&mut chain),
        benilla_formats::load_pet_loyalty_names(&mut chain),
    ) {
        (Ok(personalities), Ok(loyalty)) => {
            info!(
                "pet stat tables: {} personalities, {} loyalty levels",
                personalities.len(),
                loyalty.len()
            );
            commands.insert_resource(crate::ui_pet_stats::PetStatTables {
                personalities,
                loyalty,
            });
        }
        // Without them happiness answers nil (the icon hides) and the loyalty line is blank; the
        // rest of the pet page reads the descriptor.
        (p, l) => warn!(
            "pet stat tables unavailable, happiness/loyalty stay blank: {:#}",
            p.err().or(l.err()).expect("at least one of the two failed")
        ),
    }
    match (
        benilla_formats::load_creature_families(&mut chain),
        benilla_formats::load_pet_food_names(&mut chain),
    ) {
        (Ok(families), Ok(foods)) => {
            info!(
                "pet family tables: {} families, {} food types",
                families.len(),
                foods.len()
            );
            commands.insert_resource(crate::ui_pet_stats::PetFamilyTables { families, foods });
        }
        // Without them `UnitCreatureFamily("pet")` is nil, which the stock page's guard turns into
        // a blank level line (`PetPaperDollFrame.lua:68-70`), and the diet tooltip is empty.
        (f, p) => warn!(
            "pet family tables unavailable, the pet's level line stays blank: {:#}",
            f.err().or(p.err()).expect("at least one of the two failed")
        ),
    }
    match CharacterGeosets::load(&mut chain) {
        Ok(geosets) => commands.insert_resource(Characters(geosets)),
        Err(e) => warn!("character geosets unavailable, players show every geoset: {e:#}"),
    }
    match CharSections::load(&mut chain) {
        Ok(sections) => commands.insert_resource(SkinSections(sections)),
        Err(e) => warn!("char sections unavailable, player bodies stay untextured: {e:#}"),
    }
    match CharCreateCatalog::load(&mut chain) {
        Ok(catalog) => commands.insert_resource(CharCreate(catalog)),
        Err(e) => warn!("char-create catalog unavailable, the create screen is disabled: {e:#}"),
    }
    match load_item_display_catalog(&mut chain) {
        Ok(catalog) => {
            info!("item display catalog: {} entries", catalog.len());
            commands.insert_resource(ItemDisplays {
                catalog,
                models: HashMap::new(),
            });
        }
        Err(e) => warn!("item displays unavailable, units hold nothing: {e:#}"),
    }
    match benilla_formats::load_item_visual_catalog(&mut chain) {
        Ok(visuals) => {
            info!("item visual catalog: {} glow rows", visuals.len());
            commands.insert_resource(ItemGlows::new(visuals));
        }
        Err(e) => warn!("item visuals unavailable, weapons never glow: {e:#}"),
    }
    // `SpellItemEnchantment`: the enchant glow and the tooltip's enchant line.
    match benilla_formats::load_enchant_catalog(&mut chain) {
        Ok(enchants) => {
            info!(
                "enchant catalog: {} named, {} carrying a glow",
                enchants.name_count(),
                enchants.visual_count()
            );
            commands.insert_resource(crate::items::Enchants(enchants));
        }
        Err(e) => warn!("enchants unavailable: no enchant glow, no enchant line: {e:#}"),
    }
    // `ItemRandomProperties`: a random suffix's name and its enchant slots 2..6.
    match benilla_formats::load_random_property_catalog(&mut chain) {
        Ok(props) => {
            info!("random-property catalog: {} suffix rows", props.len());
            commands.insert_resource(crate::items::RandomProperties(props));
        }
        Err(e) => {
            warn!("random properties unavailable: no item name suffix, no suffix line: {e:#}")
        }
    }
    match benilla_formats::load_durability_tables(&mut chain) {
        Ok(tables) => commands.insert_resource(crate::ui_merchant::RepairTables(tables)),
        Err(e) => warn!("durability tables unavailable, repair costs show 0: {e:#}"),
    }
    match benilla_formats::load_bank_bag_slot_prices(&mut chain) {
        Ok(prices) => commands.insert_resource(crate::ui_bank::BankPrices(prices)),
        Err(e) => warn!("bank bag slot prices unavailable, the purchase row shows 0: {e:#}"),
    }
    match benilla_formats::load_stable_slot_prices(&mut chain) {
        Ok(prices) => commands.insert_resource(crate::ui_stable::StableSlotPrices(prices)),
        Err(e) => warn!("stable slot prices unavailable, the purchase row shows 0: {e:#}"),
    }
    match benilla_formats::load_stationery_catalog(&mut chain) {
        Ok(catalog) => commands.insert_resource(crate::ui_mail::Stationery(catalog)),
        Err(e) => warn!("stationery catalog unavailable, mail uses the default backdrop: {e:#}"),
    }
    match benilla_formats::load_page_text_material_catalog(&mut chain) {
        Ok(catalog) => commands.insert_resource(crate::ui_item_text::PageMaterials(catalog)),
        Err(e) => warn!("page text materials unavailable, books read on parchment: {e:#}"),
    }
}

/// Ensure a [`DisplayModel`] for every display id in use and build its parts once the model loads.
fn update_display_models(
    // For corpses: `CORPSE_FLAG_BONES` decides which cache holds the model.
    entities: Query<(&NetEntity, Option<&crate::net::ObjectStore>)>,
    mut creatures: Option<ResMut<Creatures>>,
    mut gameobjects: Option<ResMut<GameObjects>>,
    mut held: Option<ResMut<ItemDisplays>>,
    mut spell_fx: Option<ResMut<spell_fx::SpellFx>>,
    mut glows: Option<ResMut<ItemGlows>>,
    model_assets: (Res<Assets<M2Model>>, Res<Assets<WmoModel>>),
    mut forms: ResMut<benilla_world::model_forms::ModelForms>,
    asset_server: Res<AssetServer>,
    mut mats: benilla_world::model_render::M2BatchMaterials,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    mut bones: ResMut<corpse::BonesModels>,
    // The glue screens' body display, wanted with no wire entity.
    glue_preview: Option<Res<crate::portrait::GluePreview>>,
    char_create: Option<Res<CharCreate>>,
    // The stable pane's pet (`SetPetStablePaperdoll`, `0x4cb870`), with no wire entity.
    stable_booth: Option<Res<crate::portrait::StableBooth>>,
) {
    if !mats.ready() {
        return; // no lighting yet → no materials to build
    }
    let (m2s, wmos) = (&model_assets.0, &model_assets.1);
    // Shared per display, so the materials belong to the batch; a GameObject that needs its own
    // clones them at spawn (`attach::dress`).
    let mut uv = benilla_world::model_render::EntityUvLane {
        reg: &mut uv_reg,
        table: &mut anim_table,
        instance: None,
    };

    let mut actives: Vec<(EntityKind, u32)> = entities
        .iter()
        // A bone pile keeps the body's display id, which the client ignores; its own model is
        // collected below.
        .filter(|(e, store)| {
            !matches!(
                corpse::corpse_model(e, *store),
                Some(corpse::CorpseModel::Bones(..))
            )
        })
        .filter_map(|(e, _)| e.display_id.map(|d| (e.kind, d)))
        .collect();
    // Bone piles want the (race, sex) cache; a flesh corpse's display id is a
    // `CreatureDisplayInfo` id, resolved below like a unit's.
    let bones_wanted: Vec<(u8, u8)> = entities
        .iter()
        .filter_map(|(e, store)| match corpse::corpse_model(e, store) {
            Some(corpse::CorpseModel::Bones(race, sex)) => Some((race, sex)),
            _ => None,
        })
        .collect();
    // The glue stage's character, which nothing streamed in.
    if let (Some(preview), Some(cc)) = (glue_preview.as_deref(), char_create.as_deref()) {
        if let Some(look) = preview.look {
            let (race, sex) = look.body();
            if let Some(disp) = cc.0.body_display(race, sex) {
                actives.push((EntityKind::Player, disp));
            }
        }
    }
    // The select screen's pet, a plain creature: its triple carries a `CreatureDisplayInfo` id.
    if let Some(preview) = glue_preview.as_deref() {
        if let Some(pet) = preview.look.and_then(|l| l.pet()) {
            actives.push((EntityKind::Unit, pet.display_id));
        }
    }

    // The stable window's selected pet, a plain creature display too; a repeat is free.
    if let Some(display) = stable_booth.as_deref().and_then(|b| b.display_id) {
        actives.push((EntityKind::Unit, display));
    }

    for (kind, disp) in actives {
        match kind {
            // A player's body resolves through the creature chain (`CreatureDisplayInfo` →
            // `CreatureModelData` → a `Character\…` model), as does a flesh corpse's
            // `CORPSE_FIELD_DISPLAY_ID` (`0x5d6759`).
            EntityKind::Unit | EntityKind::Player | EntityKind::Corpse => {
                // Peek through `&` first: `as_deref_mut` marks `Creatures` changed, and
                // `resolve_equipment`'s skip gate reads that.
                if creatures
                    .as_deref()
                    .is_some_and(|cr| cr.models.get(&disp).is_some_and(|dm| dm.parts.is_some()))
                {
                    continue;
                }
                let Some(cr) = creatures.as_deref_mut() else {
                    continue;
                };
                if !cr.models.contains_key(&disp) {
                    let dm = new_creature_display(&cr.catalog, disp, &asset_server);
                    cr.models.insert(disp, dm);
                }
                if let Some(dm) = cr.models.get_mut(&disp) {
                    if dm.parts.is_none() {
                        build_parts(
                            dm,
                            m2s,
                            wmos,
                            &mut forms,
                            &asset_server,
                            &mut mats,
                            &mut uv,
                            false, // gameobject: no hull collider, no bake variant
                        );
                    }
                }
            }
            EntityKind::GameObject => {
                // The same peek, though no gate reads this change.
                if gameobjects
                    .as_deref()
                    .is_some_and(|go| go.models.get(&disp).is_some_and(|dm| dm.parts.is_some()))
                {
                    continue;
                }
                let Some(go) = gameobjects.as_deref_mut() else {
                    continue;
                };
                if !go.models.contains_key(&disp) {
                    let dm = new_gameobject_display(&go.catalog, disp, &asset_server);
                    go.models.insert(disp, dm);
                }
                if let Some(dm) = go.models.get_mut(&disp) {
                    if dm.parts.is_none() {
                        build_parts(
                            dm,
                            m2s,
                            wmos,
                            &mut forms,
                            &asset_server,
                            &mut mats,
                            &mut uv,
                            true, // gameobject: hull collider and the interior bake variant
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // Each wanted `(race, sex)` skeleton, built as its M2 lands. Its path needs the race's file
    // string from the char-create data; without it there are no skeletons.
    if let Some(cc) = char_create.as_deref() {
        for key in bones_wanted {
            corpse::ensure_bones_display(&mut bones, &cc.0, key, &asset_server);
            if let Some(dm) = bones.0.get_mut(&key) {
                if dm.parts.is_none() {
                    build_parts(
                        dm,
                        m2s,
                        wmos,
                        &mut forms,
                        &asset_server,
                        &mut mats,
                        &mut uv,
                        false, // gameobject: a prop body, unit lighting, no hull collider
                    );
                }
            }
        }
    }

    // Held items, effects and glows: each cache is borrowed `&mut` only while it holds an unbuilt
    // entry, or it would read as changed every frame.
    fn unbuilt<K>(models: &HashMap<K, DisplayModel>) -> bool {
        models.values().any(|dm| dm.parts.is_none())
    }
    if held.as_deref().is_some_and(|h| unbuilt(&h.models)) {
        if let Some(held) = held.as_deref_mut() {
            for dm in held.models.values_mut() {
                if dm.parts.is_none() {
                    build_parts(
                        dm,
                        m2s,
                        wmos,
                        &mut forms,
                        &asset_server,
                        &mut mats,
                        &mut uv,
                        false, // gameobject: held items, unit lighting, no collider
                    );
                }
            }
        }
    }

    // Spell-effect models, keyed by model path.
    if spell_fx.as_deref().is_some_and(|f| unbuilt(&f.models)) {
        if let Some(fx) = spell_fx.as_deref_mut() {
            for dm in fx.models.values_mut() {
                if dm.parts.is_none() {
                    build_parts(
                        dm,
                        m2s,
                        wmos,
                        &mut forms,
                        &asset_server,
                        &mut mats,
                        &mut uv,
                        false, // gameobject: effects, unit lighting, no collider
                    );
                }
            }
        }
    }

    // Item and enchant glow models, path-keyed like the effects.
    if glows.as_deref().is_some_and(|g| unbuilt(&g.models)) {
        if let Some(glows) = glows.as_deref_mut() {
            for dm in glows.models.values_mut() {
                if dm.parts.is_none() {
                    build_parts(
                        dm,
                        m2s,
                        wmos,
                        &mut forms,
                        &asset_server,
                        &mut mats,
                        &mut uv,
                        false, // gameobject: effects, unit lighting, no collider
                    );
                }
            }
        }
    }
}

/// Write a unit part's animated colour alpha into its `MeshTag` alpha field, the dimming factor of
/// the per-batch `A = instanceAlpha × colourAlpha × weight` (`0x707680`); the `A ≤ 0` cull is the
/// `Visibility` authority's. Written verbatim, never multiplied in, after the interior classifier;
/// a part under an appear or despawn fade is `apply_render_fade`'s, which folds the factor in.
#[allow(clippy::type_complexity)]
fn apply_unit_mat_alpha(
    mut parts: Query<
        (
            &benilla_world::doodad_anim::MatAnim,
            &mut bevy::mesh::MeshTag,
        ),
        (
            Without<benilla_world::model_fade::RenderFade>,
            Without<benilla_world::model_fade::PendingAppearFade>,
        ),
    >,
    mut logged: Local<bool>,
) {
    let mut culled = 0usize;
    for (anim, mut tag) in &mut parts {
        if !anim.composes_unit_tag() {
            continue;
        }
        if anim.current <= 0.0 {
            culled += 1;
        }
        let bits = benilla_world::mesh_tag::with_alpha(tag.0, anim.current);
        if tag.0 != bits {
            tag.0 = bits;
        }
    }
    // Logged once, the first frame a unit batch reaches the `A <= 0` cull; the debug panel's
    // material meter has the live count.
    if !*logged && culled > 0 {
        *logged = true;
        info!("unit material alpha: {culled} batch(es) culled at A <= 0 (the first frame any did)");
    }
}

#[cfg(test)]
mod world_unit_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn net(kind: EntityKind, scale: f32) -> NetEntity {
        NetEntity {
            kind,
            display_id: None,
            scale,
        }
    }

    #[test]
    fn every_wire_body_becomes_a_world_unit_and_the_viewer_is_marked() {
        let mut app = App::new();
        let plain = app.world_mut().spawn(net(EntityKind::Unit, 1.25)).id();
        let chest = app.world_mut().spawn(net(EntityKind::GameObject, 1.0)).id();
        let me = app
            .world_mut()
            .spawn((
                net(EntityKind::Player, 1.0),
                crate::net::Embodied,
                collision_height::CollisionHeight(2.5),
            ))
            .id();

        app.world_mut()
            .run_system_once(publish_world_units)
            .expect("reconciler runs");

        let w = app.world();
        let unit = w
            .get::<benilla_world::world_unit::WorldUnit>(plain)
            .expect("a wire body is a world unit");
        assert!(unit.wades, "a creature displaces water");
        assert_eq!(unit.scale, 1.25);
        assert_eq!(
            unit.height,
            collision_height::CollisionHeight::default().0,
            "an unresolved height reads as the CLIENT's ctor default, never 0.0 — at zero every \
             depth line collapses and the body swims on dry land (see the type's own doc)"
        );

        // `benilla-world` has no `TYPEID`: only the reconciler tells a creature from a GameObject.
        assert!(
            !w.get::<benilla_world::world_unit::WorldUnit>(chest)
                .expect("a GameObject is still a body the world can shade and room-claim")
                .wades,
            "…but a chest standing in a lake makes no ripple"
        );

        let mine = w
            .get::<benilla_world::world_unit::WorldUnit>(me)
            .expect("so is the avatar's");
        assert!(mine.wades, "and so does a player");
        assert_eq!(mine.height, 2.5, "the collision cylinder travels with it");
        assert!(
            w.get::<benilla_world::world_unit::ViewerUnit>(me).is_some(),
            "and the eye's own body is marked, which is what first-person feathering filters on"
        );
        assert!(
            w.get::<benilla_world::world_unit::ViewerUnit>(plain)
                .is_none(),
            "…and nothing else is"
        );
    }

    #[test]
    fn the_viewer_marker_follows_self_player_off_as_well_as_on() {
        let mut app = App::new();
        let me = app
            .world_mut()
            .spawn((net(EntityKind::Player, 1.0), crate::net::Embodied))
            .id();
        app.world_mut()
            .run_system_once(publish_world_units)
            .expect("reconciler runs");
        assert!(app
            .world()
            .get::<benilla_world::world_unit::ViewerUnit>(me)
            .is_some());

        app.world_mut()
            .entity_mut(me)
            .remove::<crate::net::Embodied>();
        app.world_mut()
            .run_system_once(publish_world_units)
            .expect("reconciler runs");
        assert!(
            app.world()
                .get::<benilla_world::world_unit::ViewerUnit>(me)
                .is_none(),
            "the marker is reconciled off, not left behind"
        );
    }
}

#[cfg(test)]
mod display_mirror_tests {
    use super::*;
    use crate::portrait::PortraitSeat;

    /// One synthetic batch, plain or camera-facing on `bone`.
    fn part(billboard: Option<u16>) -> crate::entities::display::EntityPart {
        crate::entities::display::EntityPart {
            mesh: Handle::default(),
            geometry: std::sync::Arc::new(benilla_formats::RenderSubmesh::default()),
            aabb: None,
            skinned_mesh: Some(Handle::default()),
            welded_billboard: false,
            material: Handle::default(),
            material_interior: None,
            material_interior_bake: None,
            material_interior_bake_blend: None,
            fade_blend: None,
            zfill: None,
            blend: benilla_formats::ModelBlend::Opaque,
            additive: false,
            two_sided: false,
            geoset_id: 0,
            char_slot: None,
            billboard: billboard.map(|bone| benilla_assets::BillboardInfo {
                pivot: Vec3::ZERO,
                bone,
                kind: benilla_formats::BillboardKind::Spherical,
                scale_anim: None,
                seq_translations: Vec::new(),
            }),
            alpha_anim: None,
            rgb_anim: None,
            rgb_seq: None,
            uv_anim: None,
            uv_seq: None,
            uv_rot_seq: None,
            uv_scale_seq: None,
            ground_quad: None,
        }
    }

    fn creatures(display_id: u32, dm: crate::entities::display::DisplayModel) -> Creatures {
        let mut c = Creatures {
            catalog: Default::default(),
            models: HashMap::new(),
        };
        c.models.insert(display_id, dm);
        c
    }

    #[test]
    fn a_display_mirror_carries_every_batch_and_splits_the_camera_facing_ones() {
        let mut dm = crate::entities::display::empty_shell();
        dm.parts = Some(vec![part(None), part(Some(17)), part(None)]);
        let c = creatures(4449, dm);

        let (parts, cards) = c.display_mirror(4449).expect("the model is built");
        assert_eq!(parts.len(), 2, "the two plain batches are ordinary parts");
        assert_eq!(
            cards.len(),
            1,
            "the camera-facing batch is a card, not a part"
        );
        assert_eq!(cards[0].bone, 17, "the card keeps its own billboard bone");
        assert_eq!(
            cards[0].seat,
            PortraitSeat::Body,
            "a creature's own batch is the host model's — its bone's booth joint already bakes the \
             pivot, so it takes no rider offset"
        );
        assert!(
            cards[0].attach.is_none(),
            "a batch of the host model itself sits in no M2 attachment node"
        );
        assert!(
            parts.iter().all(|p| p.skinned_mesh.is_some()),
            "the skinned twin travels — the booth poses it at Stand on its own skeleton"
        );
    }

    #[test]
    fn a_display_still_loading_mirrors_nothing() {
        let c = creatures(4449, crate::entities::display::empty_shell());
        assert!(c.display_mirror(4449).is_none(), "parts not built yet");
        assert!(c.display_mirror(1).is_none(), "unknown display");
    }
}

#[cfg(test)]
mod overhead_slot_tests {
    use super::*;

    /// A body model authoring exactly `slots`.
    fn body(slots: &[u16]) -> BoneAttach {
        BoneAttach {
            points: slots.iter().map(|&s| (s, (0u16, Vec3::ZERO))).collect(),
            markers: HashMap::new(),
        }
    }

    /// `0x6074c0`'s pick. A character body (HumanMale) authors both slots, a Kobold only 18.
    #[test]
    fn twenty_nine_needs_both_a_mount_model_and_the_authored_point() {
        let both = body(&[ATTACH_OVERHEAD, ATTACH_OVERHEAD_MOUNTED]);
        assert_eq!(overhead_slot(&both, false), Some(ATTACH_OVERHEAD));
        assert_eq!(overhead_slot(&both, true), Some(ATTACH_OVERHEAD_MOUNTED));

        let plain = body(&[ATTACH_OVERHEAD]);
        assert_eq!(overhead_slot(&plain, false), Some(ATTACH_OVERHEAD));
        assert_eq!(
            overhead_slot(&plain, true),
            Some(ATTACH_OVERHEAD),
            "no 29 on the body ⇒ the mounted leg falls back to 18, like the client"
        );

        let mounted_only = body(&[ATTACH_OVERHEAD_MOUNTED]);
        assert_eq!(
            overhead_slot(&mounted_only, false),
            None,
            "29 is never the unmounted fallback — the fallback is 18 or nothing"
        );
        assert_eq!(
            overhead_slot(&mounted_only, true),
            Some(ATTACH_OVERHEAD_MOUNTED)
        );

        assert_eq!(overhead_slot(&body(&[]), true), None, "never parented");
    }
}

#[cfg(test)]
mod attach_bias_tests {
    use super::*;

    /// The table at `0x862708`, verbatim; animation events reach it by ids 17 and 1.
    #[test]
    fn the_z_bias_table_is_the_reference_row_for_ever_id() {
        assert_eq!(ATTACH_Z_BIAS.len(), 0x25, "ids 0..=0x24");
        assert_eq!(ATTACH_Z_BIAS[17], 2.0, "$CSD's fallback lift");
        assert_eq!(ATTACH_Z_BIAS[1], 1.0, "a whiffed $CSS's");
        // Values other than 1.0 and 1.5, where a transcription slip shows.
        assert_eq!(ATTACH_Z_BIAS[19], 0.0);
        assert_eq!(ATTACH_Z_BIAS[29], 3.0);
        assert_eq!(ATTACH_Z_BIAS[18], 2.5);
        // Out of range reads as no lift, matching the reference's `0 ≤ id < 0x25` guard.
        assert_eq!(ATTACH_Z_BIAS.get(0x25), None);
    }
}
