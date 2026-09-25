//! Streamed world entities: give each network entity ([`crate::net::NetEntity`]) a visual. NPCs **and
//! other players** render as their real M2 (resolved from the display id via `CreatureCatalog` — a
//! player's body resolves through the same creature chain), GameObjects as their model
//! (`GameObjectCatalog`), and everything else as a colored cube.
//!
//! The entity itself — its identity, pose, and movement — is owned by the net bridge: `apply_net_updates`
//! spawns one real ECS entity per server guid (with a [`Transform`] driven by `sample_splines`), and this
//! module simply attaches a visual to it as soon as the model asset has loaded. There is no per-frame
//! snapshot and no entity side-map: a unit *is* one entity.
//!
//! Models load through the standard `AssetServer` as `Handle<M2Model>`/`Handle<WmoModel>` (the same
//! `mpq://` pipeline the terrain streamer uses) — deduped + async, no main-thread parse. Per display id
//! we keep a [`DisplayModel`]: its handle + (for creatures) its skin variations, and the spawn parts
//! built once the asset loads (creature `Monster1/2/3` skin slots filled here from the display's
//! variations — see [`ModelSubmesh::skin_slot`]).

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

/// Resolving a display id to its [`DisplayModel`] and building its spawn parts once the model asset
/// loads (materials, skeleton, collider, camera/selection metrics) — the front half of this subsystem,
/// kept in its own file as it carries the bulk of the per-display cache/build logic.
pub(crate) mod display;
use display::{
    build_parts, empty_display, empty_shell, new_creature_display, new_gameobject_display,
    DisplayModel, EntityPart, ModelHandle,
};

/// Attaching a visual to each net entity (the back half of this subsystem) — kept in its own file as it
/// carries the bulk of the per-entity spawn logic (skeleton/animation, character geoset + skin, fade).
mod attach;
use attach::{attach_entity_visuals, build_dressup_preview, build_glue_pet, build_glue_preview};

/// The dynamic point lights an entity's own model carries into the world — the held torch above all:
/// decision 0016's law applied to the *entity* half of the scene, not just the placed half.
mod carried_light;
use carried_light::spawn_carried_lights;

/// Equipment visuals: held items (weapon/shield/ranged) plus worn-armor and
/// helm/shoulder resolution, all resolved from the unit descriptor + ItemDisplayInfo and spawned as
/// children of the body's attach-point joints.
mod equipment;
use equipment::{attach_held_items, resolve_corpse_equipment, resolve_equipment};

/// Item / enchant glow effects: the `Spells\Enchantments\*.mdx` models an item's
/// `ItemVisuals` id hangs on the item's OWN attachment points — the permanent weapon glows and
/// the shaman/oil enchant visuals.
mod item_glow;
pub(crate) use item_glow::ItemGlowAttached;
use item_glow::{attach_item_glows, ItemGlows};

/// Mounts: the `UNIT_FIELD_MOUNTDISPLAYID` → second-creature-visual projection —
/// the mount child + seat components and the transition's **re-seat** (the reference
/// re-parents the rider's model onto the mount, it never rebuilds it).
pub(crate) mod mount;
use mount::reseat_mounts;

/// Live descriptor appearance: a `Values` delta moving the display id swaps the
/// model in place (druid forms, GM morphs — ledger B69/F04) and one moving `SCALE_X` eases the
/// render scale (the reference's 2 s cosine smoothstep); both restamp the collision height.
/// The corpse OBJECT: a `TYPEID_CORPSE` create rendered as the dead body — the
/// character-model chain dressed from the corpse's own `CORPSE_FIELD_*` snapshot, or the
/// `<Race><Sex>DeathSkeleton` prop once the server has converted it to bones.
pub(crate) mod corpse;

mod live_display;
pub(crate) use live_display::DisplaySwapped;
use live_display::{refresh_live_display, tick_scale_ease};

/// Terrain conform: every flagged model (`GlobalModelFlags & 3 ∈ {1,3}` —
/// mounts and wild quadrupeds alike) tilts to the ground under its unit, through the conform
/// node its root bones parent under.
mod conform;

/// The per-unit collision height: the display id's `CreatureModelData` column,
/// scaled — the `h` the swim/wade/splash/foam depth lines are each a verified fraction of.
mod collision_height;
use collision_height::stamp_collision_heights;
pub(crate) use collision_height::CollisionHeight;

/// Spell-visual effect models (decision 0099 phase 3): a casting unit's attach-point `.mdx` glows,
/// spawned under the same attach-point joints as held items, lifetime per the kit stage.
mod missile;
use missile::{attach_missile_models, move_missiles, spawn_missiles};
pub(crate) use missile::{MissileMiss, MissileSound, PendingMissiles};

/// WMO-display GameObject doodad props (the ship's sails / the zeppelin's rotor): the WMO's MODD
/// M2s spawned as children of the streamed gameobject, so they ride a moving transport.
mod wmo_props;
use wmo_props::{resolve_wmo_gameobject_props, spawn_wmo_gameobject_props};
pub(crate) mod spell_fx;
use spell_fx::{attach_spell_fx, resolve_spell_fx};

/// Dest-anchored spell effects: a DynamicObject's persistent area visuals
/// (Blizzard's storm, Flamestrike's burn) + the GO dest one-shot burst. (Distinct from
/// `crate::ground_fx`, the flat-quad decal renderer this lane's models feed into.)
pub(crate) mod dest_fx;
use dest_fx::{
    arm_ground_effects, attach_ground_fx_models, spawn_ground_bursts, tick_shard_emitters,
};

/// Spell **chain beams**: the polyline of hops a kit's chain `CharProc` draws —
/// Chain Lightning's arcs, Drain Life's rope of soul, C'Thun's eye beam. Its geometry rides the
/// shared effect-quad stream beside the ribbon trails.
mod chain_beam;
pub(crate) use chain_beam::ChainHops;
use chain_beam::{simulate_chain_beams, spawn_chain_beams};
// The container feed reads the icon column off the same catalog resource (one DBC parse).
// The one InventoryType → equipment-slot table (`attach::preview`): the dressing-room feed places a
// tried-on item by the very same map the preview it feeds dresses by.
pub(crate) use attach::{equip_slot, BodyPartsDesc};
pub(crate) use equipment::ItemDisplays;
pub(crate) use equipment::{BoneAttach, Equipment};
// The instruments' read of what a body is actually WEARING vs what it resolved (`WOW_DRESS_CENSUS`).
pub(crate) use equipment::{attach_id, DressKey, HeldAttached, ATTACH_SLOT_NAMES};
// [`DressKey`] carries one of these per weapon slot, but nothing outside this module has to NAME
// the type to hold or compare a key — only the tests that build one do.
#[cfg(test)]
pub(crate) use equipment::ItemModelKind;

/// The overhead attachment slot (`PlayerName`, id 18). The anchor of the overhead name, the
/// floating combat text, and the questgiver marker alike.
pub(crate) const ATTACH_OVERHEAD: u16 = 18;

/// Its mounted twin (`PlayerNameMounted`, id 29) — preferred while a mount model is attached
/// (`0x6074c0`, `0x608640`; decision 0441 P2), authored on the RIDER's own model (character
/// models seat it higher so overhead content clears the mount's bulk). A rider model without it
/// falls back to 18 like the client.
pub(crate) const ATTACH_OVERHEAD_MOUNTED: u16 = 29;

/// The overhead attachment slot to use for a unit **right now** — the reference's own pick inside
/// the marker attach `0x6074c0`, always queried on the unit's *body* model: slot **29** when a
/// mount MODEL exists (`unit+0xdc != 0` — our [`mount::MountChild`]) *and* the body authors 29,
/// else slot **18**; `None` when the body authors neither, which is the reference's "marker
/// created but never parented" (invisible) and this file's bbox fallback for the overhead readers.
///
/// **The pick is LIVE, not a bake.** `0x6074c0`'s five call sites include `0x5ffae7` inside
/// `0x5ffa50` — the `UNIT_FIELD_MOUNTDISPLAYID` field-watch handler (`0x604330 mov edx,0x1fc; mov
/// ecx,3`), the very handler [`mount::reseat_mounts`] ports — so the reference re-runs the whole
/// attach, slot choice included, on every mount and dismount. A per-frame *reader*
/// ([`overhead_anchor`]) gets that for free; a consumer that PARENTS something at the slot
/// ([`crate::quest_markers`]) has to notice the move and re-parent itself.
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

/// The unit's **Stand-animation box height** (model-local, pre-scale) — the chat bubble's anchor,
/// and *only* the chat bubble's (1406).
///
/// The reference's two overhead heights are two different mechanisms, and benilla had recorded them
/// as one. The overhead NAME (and the floating combat text, and the V-plate) takes `0x608640`: the
/// live posed PlayerName attachment, which tracks the pose. The chat bubble takes `0x711a20`, which
/// reads the **MD20 header image** — file bytes, no bone matrix in the call tree — returning the
/// Stand sequence CAaBox's Z extent, and the client caches it in the bubble at `+0x354` on a parity
/// guard so it is queried **once per chat line**. The recorded claim that the two calls were
/// equivalent ("both are the head-region attachment height, model-scaled", INFERRED) is refuted:
/// they differ precisely on animated-vs-static.
///
/// So this is a constant per display, stamped at attach and never re-read — which is also why the
/// bubble's height cannot acquire a pose-clock bug of the kind 1398 had to remove from the anchor.
#[derive(Component)]
pub(crate) struct StandBoxHeight(pub(crate) f32);

/// The unit's model bbox z-extent (model-local, pre-scale) — stamped by the attach path for
/// [`overhead_anchor`]'s fallback. `0.0` until the model loads (the fallback then anchors at
/// feet — the same degenerate the client hits with no model).
#[derive(Component)]
pub(crate) struct OverheadFallback(pub(crate) f32);

/// The unit's OVERHEAD anchor, world space (`0x608640`, byte-read): the **posed** PlayerName
/// attachment (slot 29 while a mount model is attached, else 18 — head height, tracking model
/// stature and the live pose intrinsically), else `feet + scale × bbox_z × 1.25`. Consumed
/// per-frame by the nameplate and snapshotted at spawn by the floating combat text. Generic over
/// the joint-globals query filter so a caller that also mutates `GlobalTransform` elsewhere (the
/// nameplate placer) can pass a disjoint query.
///
/// A pure position read: it computes through `RigPose::posed_point` — the composed pose × the
/// rig root's frame — and never touches an anchor entity, so the overhead bone spawns nothing.
///
/// **The rig root's frame is taken from `tf`, not from its propagated `GlobalTransform`, whenever
/// the rig root IS the unit** — which is the normal case (a mounted rider's root is the seat
/// anchor, a conform-tilted model's is its conform node; those two still read the propagated
/// frame and keep the lag below). Bevy propagates `GlobalTransform` in `PostUpdate`, so an
/// `Update` reader like the V-plate or the chat bubble gets **last frame's** world frame while the
/// unit's own `Transform` beside it — and the camera it projects through, and the model being
/// drawn — are this frame's. Running, that seam is one frame of travel: measured on a Westfall run
/// (1398), "head above feet" — a body constant — wobbled up to **11.7 px** per frame and 15.1 px
/// across the leg, and collapsed to 0.02 px the moment the anchor was paired with the position it
/// was actually computed from. That is the chat bubble sliding against the head the director
/// reported as jitter, and 1341 cleared the same term for the plate by measuring a unit that was
/// STANDING STILL, where it is identically zero.
///
/// Reading `tf` is exact here because a unit is a world-root entity — nothing in the world lane
/// parents a `NetEntity` (the `ChildOf` sites are the portrait booth, the pipe-warm menagerie and
/// the UI glue), so its global IS its local. A `PostUpdate` caller ([`crate::nameplates`], which
/// moved there for this very lag) is unaffected: after propagation the two are the same value.
/// The client's **per-attachment z-bias** fallback table, `[0x862708]` — 37 floats, attachment
/// ids `0..=0x24`, read as `[0x862708 + 4·id]` under a `0 ≤ id < 0x25` guard. It is consulted only
/// when the model does **not** carry the attachment the caller asked for: the sound then plays at
/// the unit's own position raised by this much, which is how a headless or attachment-less model
/// still puts a mouth sound somewhere plausible instead of at its feet.
const ATTACH_Z_BIAS: [f32; 37] = [
    1.0, 1.0, 1.0, 1.5, 1.5, 1.8, 1.8, 0.5, 0.5, 1.0, 1.0, 2.0, 1.5, 1.0, 1.0, 1.0, 1.0, 2.0, 2.5,
    0.0, 2.0, 1.5, 1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 3.0, 1.5, 1.5, 1.0, 1.0, 1.5, 1.0, 1.0,
];

/// **`0x623b90(attachId)` — where a unit-attached sound is born**, as a [`SystemParam`] because
/// two different routers need the same three pure reads.
///
/// [`SystemParam`]: bevy::ecs::system::SystemParam
///
/// `AttachPoints::point` returns The attachment's live world
/// point when the model carries it (`0x712cb0` finds the record, `0x712d50` transforms it), else
/// the unit's own `GetPosition` raised by [`ATTACH_Z_BIAS`]`[attachId]` (`0x623be2 fadd
/// [4·ebx + 0x862708]`).
///
/// Two anim-event arms reach it and they are the reason this exists as a named function rather
/// than inline at either: the emote voice `$CSD` asks for **17** (`0x623c3a push 0x11`), and a
/// **whiffed** melee swing's `$CSS` asks for **1** (`0x624bdd`). Both are emphatically *not* the
/// fired event's own point, which is what makes them the exceptions among the unit's anim-event
/// arms — a player model's six head-mounted `$CSD` records do not decide where that sound plays.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AttachPoints<'w, 's> {
    anchors: Query<'w, 's, &'static BoneAttach>,
    poses: Query<'w, 's, &'static benilla_world::rig_anim::RigPose>,
    globals: Query<'w, 's, &'static GlobalTransform>,
}

impl AttachPoints<'_, '_> {
    /// The world point, with `fallback` standing in for the reference's `GetPosition` (the caller
    /// already holds the unit's transform, and passing it keeps this a pure read).
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

/// The child mesh spawned by the fallback-cube arm of [`attach::attach_entity_visuals`] — "this
/// entity's display named no model we could load".
///
/// A marker, so the condition is **countable** rather than eyeballed: `WOW_UNIT_VISUALS`
/// ([`crate::capture::probes::UnitVisualsPlugin`]) reports how many streamed entities are standing
/// as cubes, and on which displays. Before decision 1403 a cube also stood for every invisible
/// trigger creature, which is what B13 saw as a black slab — an unlit `StandardMaterial` catches no
/// light in our scene, so the "red" NPC box renders pure black. The census is how that stays
/// visible if the gate ever regresses.
#[derive(Component)]
pub(crate) struct FallbackCube;

/// Shared fallback cube mesh + per-kind materials, used when an entity has no usable model. (No
/// GameObject color: GameObjects render their model or nothing — a model-less GameObject is an
/// effect/trigger that's invisible in the real client.)
#[derive(Resource)]
pub(crate) struct CubeAssets {
    mesh: Handle<Mesh>,
    /// Slimmer, shorter block for a player whose body model isn't available (smaller than the NPC box).
    player_mesh: Handle<Mesh>,
    player_mat: Handle<StandardMaterial>,
    npc_mat: Handle<StandardMaterial>,
}

impl CubeAssets {
    /// The pipeline-warm rig parts ([`crate::pipe_warm`]): the production cube
    /// mesh + both materials, so the fallback-cube pipeline compiles behind the cover instead of
    /// on the first model-less spawn in view.
    pub(crate) fn warm_parts(&self) -> (Handle<Mesh>, [Handle<StandardMaterial>; 2]) {
        (
            self.mesh.clone(),
            [self.player_mat.clone(), self.npc_mat.clone()],
        )
    }
}

/// Creature rendering: the display→model catalog + a per-display [`DisplayModel`] cache. Optional — if
/// the DBCs fail to load, NPCs stay cubes.
#[derive(Resource)]
pub(crate) struct Creatures {
    catalog: CreatureCatalog,
    models: HashMap<u32, DisplayModel>,
}

impl Creatures {
    /// A display's **foley material** (`Material.dbc` id) — the creature half of the footfall
    /// rustle; see [`benilla_formats::CreatureCatalog::foley_material`] and
    /// [`crate::sound::footsteps`]. `None` for an unknown display.
    pub(crate) fn foley_material(&self, display_id: u32) -> Option<u32> {
        self.catalog.foley_material(display_id)
    }

    /// A display's collision height in **raw model units** — see [`CollisionHeight`] for the world
    /// value and everything that reads it. `None` for an unknown display.
    pub(crate) fn collision_height(&self, display_id: u32) -> Option<f32> {
        self.catalog.collision_height(display_id)
    }

    /// A display's footprint-decal parameters (yards, pre-scale) — `None` = this model leaves no
    /// prints. See [`benilla_formats::CreatureCatalog::footprint`].
    pub(crate) fn footprint(&self, display_id: u32) -> Option<benilla_formats::FootprintParams> {
        self.catalog.footprint(display_id)
    }

    /// Does this display's model breathe — may it wear the `$BTH` puffs (cold vapour, bubbles)?
    /// `CreatureModelData.Flags & 0x2` suppresses the family: skeletons, ghosts, elementals,
    /// golems, slimes, totems. See [`benilla_formats::CreatureCatalog::breathes`].
    pub(crate) fn breathes(&self, display_id: u32) -> bool {
        self.catalog.breathes(display_id)
    }

    /// A display's **base render alpha** (`CreatureDisplayInfo.CreatureModelAlpha / 255`) — the
    /// `baseAlpha` factor of the per-unit alpha product the aura CharProc nodes multiply into
    /// (`crate::aura_visual`). `None` for an unknown display.
    pub(crate) fn display_base_alpha(&self, display_id: u32) -> Option<f32> {
        self.catalog.display_base_alpha(display_id)
    }

    /// A display's **footstep camera-shake preset** (`CameraModelData.FootstepShakeSize` → a
    /// `CameraShakes.dbc` row id) — see [`crate::camera_shake`]. `None` for the 405 of 430 shipped
    /// models that shake nothing.
    pub(crate) fn footstep_shake(&self, display_id: u32) -> Option<u32> {
        self.catalog.footstep_shake(display_id)
    }

    /// A display's **death-thud camera-shake preset** — same table, fired on `$DTH`.
    pub(crate) fn death_thud_shake(&self, display_id: u32) -> Option<u32> {
        self.catalog.death_thud_shake(display_id)
    }

    /// A display's **audible size class** (`0 Small`..`4 Colossal`) — the axis the body-fall
    /// sample is picked on (`crate::sound::death_thud`). The display's own column overrides the
    /// model's; see [`benilla_formats::CreatureCatalog::size_class`].
    pub(crate) fn size_class(&self, display_id: u32) -> Option<u32> {
        self.catalog.size_class(display_id)
    }

    /// A display's **spawned-creature render scale** — see
    /// [`benilla_formats::CreatureCatalog::model_scale`] for why almost nothing reads it and why
    /// the glue screens' pet must. `None` for an unknown display.
    pub(crate) fn model_scale(&self, display_id: u32) -> Option<f32> {
        self.catalog.model_scale(display_id)
    }

    /// A display's two **blood-row candidates** — `CreatureDisplayInfo.BloodLevel` and
    /// `CreatureModelData.BloodID`, tiers 1 and 2 of the reference's UnitBloodLevels resolve.
    /// `None` = unknown display. The tiers themselves need the table, so they live in
    /// [`benilla_formats::BloodCatalog::level_key`] — which is where tier 3 is, and 1850's whole
    /// point is that tier 3 is not optional.
    pub(crate) fn blood_candidates(&self, display_id: u32) -> Option<(i32, i32)> {
        self.catalog
            .model(display_id)
            .map(|m| (m.blood_display, m.blood_model))
    }

    /// A built display's booth framing — the model's own **authored cameras**, which is what both
    /// booth families frame through: `camera` for the round portrait (`cameraLookup[0]`,
    /// `0x713540`) and `pane_camera` for a `<PlayerModel>` body pane (raw index 1, `0x505890`).
    /// Each carries the fallback data its own path needs when the model has no such camera: the
    /// heuristic anchors (head bone / neck height / footprint) for the portrait, the bbox centre
    /// for the pane's fixed camera. All model-local pre-scale. `None` while the display's model is
    /// still loading (the booth's part source — the attach-spawned children — won't exist yet
    /// either).
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

    /// A built display's **mirror lists** — the same `(PortraitPart, PortraitBillboard)` pair a
    /// dressed unit's attach-spawned descendants carry, assembled straight from the display cache
    /// for a creature that has **no world entity at all**: the stable window's stabled pet
    /// ([`crate::portrait::PortraitStandIn`]).
    ///
    /// The split is [`crate::entities::attach::preview`]'s `assemble_pet` law verbatim — every
    /// batch of the model, camera-facing ones separated out — because it is the same question:
    /// only the character compositor selects among geosets, and a creature is not a character
    /// model, so the world's creature path draws every batch of a beast the same way. The
    /// billboards seat at [`crate::portrait::PortraitSeat::Body`] (the model's own rigged batch:
    /// its bone's booth joint already bakes the pivot, the 0130 rig identity) with `attach: None`,
    /// which is exactly what a host model's own batch is.
    ///
    /// `None` while the display's model is still loading — the same gate [`Self::display_rig`] and
    /// [`Self::display_anchors`] apply, so all three become available together.
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

    /// A built display's **booth rig** — what the portrait booth needs to pose a fresh instance at
    /// Stand like the ref bake (`0x524f60`: a throwaway instance armed to Stand/seq-0, not the
    /// unit's live world pose): the rest skeleton, the shared inverse bind poses, and the animation
    /// surface. `None` while the model is still loading; a boneless / WMO-display model yields an
    /// empty skeleton (the booth then bakes the static bind pose).
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
    /// The shared inverse bind poses — `Some` for every M2 display (built at load), `None` only for
    /// WMO / model-less displays (whose skeletons are empty anyway).
    pub(crate) inverse_bindposes: Option<Handle<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    pub(crate) animations: Option<&'a benilla_assets::ModelAnimations>,
}

/// GameObject rendering: the GameObjectDisplayInfo catalog + a per-display [`DisplayModel`] cache. Same
/// fallback rules as [`Creatures`]; a model-less GameObject renders nothing (effect/trigger).
#[derive(Resource)]
struct GameObjects {
    catalog: GameObjectCatalog,
    models: HashMap<u32, DisplayModel>,
}

/// Character geoset selection (Milestone B): the customization → visible-geoset tables.
/// Optional — if the DBCs fail to load, players simply render every geoset (the un-filtered body), the
/// same as before this feature.
#[derive(Resource)]
struct Characters(CharacterGeosets);

/// Character skin textures (Milestone B): the CharSections base-skin lookup. Optional —
/// if it fails to load, a player's body-skin batches stay untextured (the muted fallback), as before.
#[derive(Resource)]
struct SkinSections(CharSections);

/// Character-creation source data: per-race body displayIds, race/class combos, and
/// the appearance-dial ranges. Read by the glue-preview builder ([`attach`]) — displayId +
/// look — and by the char-create screen. Optional — absent ⇒ the create screen has no data (it
/// degrades to disabled), but the rest of the game is unaffected.
#[derive(Resource)]
pub(crate) struct CharCreate(pub(crate) CharCreateCatalog);

/// Per-appearance cache of composited body-skin atlases: each distinct character look
/// composites + uploads its 256² skin once, and every player sharing that look reuses the handle. A
/// composite is a fresh `Image` asset per build (unlike an `asset_server.load` path, which dedups by
/// path), so without this cache every player would re-composite and break material dedup downstream.
#[derive(Resource, Default)]
struct SkinComposites(benilla_assets::SpatialCache<SkinKey, Handle<Image>>);

/// The appearance fields that determine a composited body skin: race/sex pick the
/// CharSections rows; skin/face/facialHair/hairStyle/hairColor pick the base + overlay variations;
/// `equip` the worn armor display ids whose region textures paint over it, by
/// bodyslot−2 — so each distinct dressed look composites + uploads once.
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
    /// The wearer's guild tabard — part of the key, not of `equip`, because two
    /// guilds' members wear the *same* tabard display and must not share one atlas. Without it the
    /// first guild to composite would lend its crest to every other guild on screen.
    pub(super) emblem: Option<benilla_formats::GuildEmblem>,
    /// The tabard designer's preview flag (1977): the emblem paints over an empty tabard slot.
    pub(super) tabard_preview: bool,
}

/// Marks a net entity whose visual (model children or fallback cube) has been attached, so the attach
/// system processes each entity exactly once. `pub(crate)`: the `waterfx` capture rig pre-marks its
/// dummy unit so it never receives the fallback cube (which would occlude the foam under test).
#[derive(Component)]
pub(crate) struct VisualAttached;

/// The per-frame entity-visuals pipeline (resolve → build → attach → held → refresh) as one set, so
/// upstream writers can order themselves before the whole chain — the animation driver's
/// [`crate::creature_anim::VisualSheath`] must land before [`resolve_equipment`] reads it, or a
/// sheath transition double-swaps the weapon placement for a frame (the "flash").
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct EntityVisualsSet;

/// [`update_display_models`]' ordering handle: anything that must **create** a display-cache entry
/// in time for this frame's build orders before it.
///
/// It exists for one caller, the `fxview` fixture's driver — which registers itself against this
/// from `capture` rather than being listed in the chain above, because a fixture's driver belongs
/// with its fixture and gameplay may not name the harness. Same shape as the
/// `waterfx` fixture's registration against the engine's `WaterFoamSet`.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DisplayBuildSet;

/// Drop every display/material dedup on a cross-map transition (`world_map::MapChange` — see its
/// doc for why a clear is always safe mid-session). These caches are get-or-insert at every use
/// site, so a cleared entry rebuilds on the next spawn that wants it; without this, every display
/// id, material key, and composited skin ever seen stayed resident for the life of the process.
fn evict_display_caches(
    mut changes: MessageReader<benilla_world::world_map::MapChange>,
    mut composites: ResMut<SkinComposites>,
    mut fx: ResMut<spell_fx::SpellFx>,
    creatures: Option<ResMut<Creatures>>,
    gos: Option<ResMut<GameObjects>>,
    items: Option<ResMut<equipment::ItemDisplays>>,
    glows: Option<ResMut<ItemGlows>>,
    // The bone-pile bodies — 16 at most, but they hold M2 handles like any other
    // display cache and must die with the map for the same reason (the teleport asset leak).
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

/// Expire the streamed-entity dedups by **distance** — the within-map half of
/// [`evict_display_caches`]. The composited skins are the notable half: each distinct dressed look is
/// its own 256² `Image`, so a session that meets a lot of players accumulates
/// uploads no map change ever reaches. The display-id → model caches (`Creatures`/`GameObjects`/
/// `ItemDisplays`/`SpellFx`) are deliberately *not* swept: they key on ids, not places, and hold M2
/// assets whose count is bounded by the catalogs rather than by where you have been.
fn scope_entity_art(
    mut scope: benilla_world::art_scope::ArtScope,
    mut composites: ResMut<SkinComposites>,
) {
    scope.apply(&mut composites.0, benilla_world::art_scope::ArtSlot::Skins);
}

/// **The armed idle's authored CAaBox for a built body**, model space — recorded by
/// [`attach_entity_visuals`] where the display model is read, and restated onto
/// [`WorldUnit::bound`](benilla_world::world_unit::WorldUnit::bound) by [`publish_world_units`].
///
/// Split from the field it feeds for the same reason `CollisionHeight` is: the attach knows the
/// number, the reconciler owns the component, and one writer per component is. Absent
/// until a body's model resolves — and on a body that never gets one, absent for good.
#[derive(Component, Clone, Copy)]
pub(crate) struct ModelBound(pub(crate) bevy::camera::primitives::Aabb);

/// Everything [`publish_world_units`] reads to restate one body: its wire record, the two
/// game-side components whose numbers it folds in, whether its `Visibility` is the transport
/// tick's, and what it currently says.
type WireBody = (
    Entity,
    &'static NetEntity,
    Option<&'static collision_height::CollisionHeight>,
    Option<&'static ModelBound>,
    Has<crate::transport::TransportAnchor>,
    Option<&'static benilla_world::world_unit::WorldUnit>,
);

/// One viewer-reconcile row: the entity, whether it is the embodied self, whether it is marked.
type ViewerRow = (
    Entity,
    Has<crate::net::Embodied>,
    Has<benilla_world::world_unit::ViewerUnit>,
);

/// Only entities that can be a viewer at all — one of the two markers present.
type ViewerCandidate = Or<(
    With<crate::net::Embodied>,
    With<benilla_world::world_unit::ViewerUnit>,
)>;

/// The edges on which a body's [`WorldUnit`](benilla_world::world_unit::WorldUnit) restatement can
/// move — [`publish_world_units`]' query filter (the anchor's removal is read separately).
type WireBodyMoved = Or<(
    Changed<NetEntity>,
    Changed<collision_height::CollisionHeight>,
    Changed<ModelBound>,
    Added<crate::transport::TransportAnchor>,
)>;

/// **Restate every wire body as a [`WorldUnit`](benilla_world::world_unit::WorldUnit)** — the game's half
/// of the unit inversion (see that module).
///
/// One reconciler rather than a line at each of the dozen sites that spawn a `NetEntity`: a
/// spawn path that forgets the marker is a body the world cannot see — no ground shade, no room
/// claim, no foam — and that is exactly the kind of omission nobody notices until a screenshot.
/// Runs between the wire drain and the rest of the frame, so a unit that arrived this frame is
/// visible to the world this frame.
fn publish_world_units(
    mut commands: Commands,
    // Only bodies whose inputs moved this frame: the restatement is a pure function of these
    // four, and walking every resident body to compare equal answers was ~1.3 k struct builds
    // a frame at the Stormwind pin (decision 1979's floor). A body leaving its transport loses
    // the anchor without a `Changed` — that edge is read off `unanchored`.
    bodies: Query<WireBody, WireBodyMoved>,
    all_bodies: Query<WireBody>,
    mut unanchored: RemovedComponents<crate::transport::TransportAnchor>,
    // Only entities that can be a viewer at all: the reconcile below needs one of the two
    // present, and an unfiltered query walked every entity in the world each frame.
    viewers: Query<ViewerRow, ViewerCandidate>,
) {
    let freed: Vec<Entity> = unanchored.read().collect();
    let due = bodies
        .iter()
        .chain(freed.iter().filter_map(|&e| all_bodies.get(e).ok()));
    for (entity, net, height, bound, anchored, current) in due {
        let want = benilla_world::world_unit::WorldUnit {
            // The wire kind is answered HERE and never handed over (1177): the engine asks "does
            // this body displace water", and translating its own vocabulary into that answer is
            // the game's job. A live body wades; a GameObject, a dynamic-object spell anchor or
            // anything else standing in a lake makes no ripple.
            wades: matches!(net.kind, EntityKind::Unit | EntityKind::Player),
            scale: net.scale,
            // `unwrap_or_default`, NOT zero: `CollisionHeight`'s Default is the client's own
            // ctor value, and its doc says why — at 0.0 "every depth line collapses and the unit
            // swims on dry land". A body whose display has not resolved yet must read as a
            // default-sized body, which is what the foam site did before this component existed.
            height: height.copied().unwrap_or_default().0,
            // The box the exterior cull may elect this body by. **A transport
            // answers `None` on purpose**: `transport::tick_transports` writes that root's
            // `Visibility` every frame off its own timetable, and a second writer there is the
            // fight decision 0025 exists to prevent — so the world is told not to decide, rather
            // than told a box and left to race.
            //
            // Every other body is elected from its FIRST frame, with a degenerate box at its own
            // origin until its model resolves. Waiting for the extent looks conservative and is
            // not: the bound reaches this reconciler only after `attach_entity_visuals` has already
            // spawned the visual, so "no box yet ⇒ admit" drew every streaming mob for one whole
            // frame through a sealed ceiling — and a cavern runs slowly enough that one frame is
            // most of a second. The origin is the server's own position, exact from the start.
            bound: (!anchored).then(|| {
                bound.map_or_else(
                    || bevy::camera::primitives::Aabb::from_min_max(Vec3::ZERO, Vec3::ZERO),
                    |b| b.0,
                )
            }),
        };
        // Only write on a real change: the component is change-detected downstream, and a
        // per-frame rewrite would mark every body dirty for every reader every frame.
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
    // The viewer marker is reconciled independently of `NetEntity`: the self entity exists before
    // its wire record does, and a `/logout` takes the record away first.
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

/// The streamed-entity subsystem: builds the shared cube assets + display catalogs at startup, then
/// each frame resolves/builds display models and attaches a visual to every net entity.
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
        // The 16 bone-pile body models, keyed (race, sex) rather than by a
        // display id — a skeleton has no CreatureDisplayInfo row.
        .init_resource::<corpse::BonesModels>()
        .init_resource::<spell_fx::SpellFx>()
        .init_resource::<spell_fx::FxTintAnims>()
        // The per-caster pending-projectile queues (the client's `unit+0xac` lists).
        .init_resource::<missile::PendingMissiles>()
        // The projectile flight-loop edges (`crate::sound::missile` consumes them).
        .add_message::<MissileSound>()
        // A travelling spell's DEFERRED outcome word (`crate::combat_text`).
        .add_message::<MissileMiss>()
        // The cast router's dest one-shot orders (`dest_fx`).
        .add_message::<dest_fx::GroundBurst>()
        // A live display-id swap's rebuild edge — consumed by the morph-latch replay
        // (`crate::creature_anim`), the reference's `0x60abe0` impact-kit replay tail.
        .add_message::<live_display::DisplaySwapped>()
        .add_systems(Startup, setup_entities.after(AssetSet::Open))
        // The map-scope teardown (`world_map::MapChange`): drop every display/material dedup
        // so a map's assets actually die with it — the teleport leak.
        .add_systems(Update, (evict_display_caches, scope_entity_art))
        // Every streamed unit's collision height, the frame after `apply_net_updates` spawns it
        // (that stage's Commands are what create the entity, so this cannot be earlier). Its
        // consumers all read `Option<&CollisionHeight>` against the ctor default, so a unit's
        // first frame simply uses that — see `CollisionHeight`.
        .add_systems(Update, stamp_collision_heights.after(WorldStage::Net))
        // Resolve/build display models before attaching, so a model is ready the frame attach wants it.
        // **After `WorldStage::Net`**: `apply_net_updates` spawns the entities (via Commands), so
        // these must run *after* that stage — with the sync point it forces — or they'd attach an
        // entity the same frame its display id is still unresolved (`dm == None`) and lock in a cube.
        // The net stage was previously unordered (`schedule.rs` chained only Input→Stream→Present), so
        // this held only by luck of Bevy's auto-sort; pinning it makes display resolution deterministic.
        .add_systems(
            Update,
            (
                // Equipment resolution first: it *creates* item
                // DisplayModel entries, which update_display_models then builds the same frame.
                // The corpse's dress rides beside it (one nested unordered
                // element — the outer tuple is at `chain()`'s 20-element ceiling): a disjoint
                // entity set, the same item-model cache, the same must-be-before-the-build.
                (resolve_equipment, resolve_corpse_equipment),
                // Spell-effect resolution likewise creates its path-keyed entries (0099 P3),
                // as does the missile launcher (P4) — both feed the same SpellFx model cache.
                resolve_spell_fx,
                move_missiles,
                spawn_missiles,
                // The dest-anchored lane: the dynobj arm + the GO burst + the shard
                // tick all create cache entries too — before the same-frame build below.
                // One nested (unordered) element: three independent producers, and the
                // outer tuple is at `chain()`'s 20-element ceiling.
                (arm_ground_effects, spawn_ground_bursts, tick_shard_emitters),
                update_display_models.in_set(DisplayBuildSet),
                // The char-create preview: assemble the selected look's parts from
                // the freshly-built display model, for the create booth to bake. After
                // `update_display_models` (its want-list built the body) — server-less, at char select.
                build_glue_preview,
                // …and the select screen's PET beside it: an ordinary creature display, assembled
                // off the same freshly-built cache, on its own latch so a slow pet model never
                // holds the character back.
                build_glue_pet,
                // The dressing room's preview: the same tuple-driven assembly,
                // for the item nobody in the world is wearing. Beside the glue one — same
                // dependency (the display cache built by `update_display_models` above), same
                // retry-until-ready latch.
                build_dressup_preview,
                attach_entity_visuals,
                // WMO-gameobject doodad props (the ship's sails): resolve the MODD list the
                // frame after the WMO visual attaches, then spawn each prop as its M2 lands
                // (parented under the entity — they sail with the boat).
                resolve_wmo_gameobject_props,
                spawn_wmo_gameobject_props,
                attach_held_items,
                // One nested (unordered) element — the two passes that hang effect models on
                // a unit: its spell-kit instances, and the glows its held items' `ItemVisuals`
                // ids name (after `attach_held_items`, which makes the item roots).
                // Independent of each other, both before the tint tick below; the outer tuple
                // is at `chain()`'s 20-element ceiling.
                (attach_item_glows, attach_spell_fx),
                // One nested (unordered) element — two independent free-model attach
                // passes, plus the world-plant tender (sweeps orphaned plants,
                // re-plants root-aura ones on owner displacement); the outer tuple is at
                // `chain()`'s 20-element ceiling.
                (
                    attach_missile_models,
                    attach_ground_fx_models,
                    spell_fx::tend_world_plants,
                ),
                // Tick the live per-instance tint clones AFTER the attach passes registered
                // them, so a clone's first drawn frame is already on its own clock.
                spell_fx::tick_fx_tint,
                // Fire each live instance's crossed event keyframes — after
                // attach so a just-spawned instance's head window [0, cur] fires this frame.
                // Beside it (one nested, unordered element — the outer tuple is at `chain()`'s
                // 20-element ceiling): the effect-model completion callbacks, which advance an
                // instance birth → Hold and arm the reap's Decay. Independent of the event
                // scan; both only read a player Bevy already advanced in `PreUpdate`, and both
                // want to run after the attach passes so a fresh instance is covered at once.
                (spell_fx::fire_fx_anim_events, spell_fx::advance_fx_anim),
                // One nested (unordered) element — the outer tuple is at `chain()`'s
                // 20-element ceiling. Two independent post-attach touch-ups on a standing
                // visual: a gear change re-dresses it in place — a re-composited atlas on the
                // same parts, the equipment geosets re-selected, every attachment left alone
                // (the reference's own shape) — and a freshly attached corpse
                // arms its settled Dead/Drowned pose.
                (attach::redress_player_looks, corpse::pose_corpses),
                // A mount transition does the same, and for the same reason: the
                // field diff **re-seats** the standing rig — onto the mount's attachment-0
                // joint, or back onto its own frame — where it used to tear the whole rider
                // down and let attach rebuild it. The reference re-parents the body model
                // (`0x712f70`/`0x713020`); it never re-creates it.
                reseat_mounts,
                // A live display-id / scale change: the display swap is the
                // same teardown-and-rebuild; the scale change arms the reference's 2 s ease.
                // The rig heal rides the same nest (Bevy's 20-tuple ceiling):
                // a unit denied a palette rig at attach rebuilds — the same teardown — once
                // the table has headroom again; never a permanent statue.
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
        // The chain-beam spawner: in the visuals set, so the cast router — which is
        // `.before(EntityVisualsSet)` — has already emitted this frame's beam plays, and the
        // net stage's Commands (the hop arrays) have already been applied.
        .add_systems(
            Update,
            spawn_chain_beams
                .in_set(EntityVisualsSet)
                .after(WorldStage::Net),
        )
        // …and its per-frame geometry, beside the ribbon trails and for the same reason: a
        // beam's endpoints are attachment joints, so it must sample the pose the billboard
        // palette and the rig finalizer just wrote.
        //
        // `.after(begin_effect_frame)` is the load-bearing one, and its omission is what made
        // the whole beam invisible on first ship: the clear carries an extra
        // `.after(face_billboards)` the beam sim does not, so without this edge the sim
        // becomes runnable a step EARLIER and its vertices are wiped before extract — every
        // frame, silently, with the arithmetic perfect. Every writer into the shared stream
        // declares this; the tripwire in `commit` now refuses a write that precedes the clear.
        .add_systems(
            PostUpdate,
            simulate_chain_beams
                .in_set(benilla_world::billboard::BillboardPlace)
                .after(benilla_world::billboard::billboard_joint_palette)
                .after(benilla_world::rig_anim::finalize_rig_worlds)
                .after(benilla_world::particles::buffer::begin_effect_frame),
        )
        // Terrain conform (the byte law of `0x614cd0` → `0x7106c0`):
        // reads each flagged unit's Update-final transform, writes its conform node's
        // local rotation — before propagation so the composite's globals carry this
        // frame's stance.
        .add_systems(
            PostUpdate,
            conform::conform_units.before(bevy::transform::TransformSystems::Propagate),
        )
        // The unit lane's material-alpha compose: after every steady-state owner of the
        // render-alpha field (the interior classifier, the visibility authority's own
        // fade/effect writes), before the self-avatar feather that is allowed to override it.
        .add_systems(
            Update,
            apply_unit_mat_alpha
                .after(benilla_world::interior::classify_entity_interior)
                .after(benilla_world::model_render::ModelVisSet)
                .after(apply_render_fade)
                .before(crate::player::apply_self_model_fade),
        )
        // The aura CharProc layer (`crate::aura_visual`): the state kit's effect on the BODY.
        // The drain installs/removes this frame's nodes; the author then owns the render alpha
        // of every part under a translucent unit — after the three steady-state alpha authors
        // above so its override lands, before the self feather which folds the factor in itself.
        .add_systems(
            Update,
            (
                // The display's own base alpha first (the reference's DISPLAYID-watcher leg
                // `0x604990` of the same recompute `0x60d180` — no aura needed), so a
                // same-frame aura edge retargets from the already-updated base.
                crate::aura_visual::refresh_base_alpha,
                crate::aura_visual::drain_aura_procs,
                // The tint publish only needs the drain ahead of it (it writes a resource, not
                // the alpha channel), so it rides the same chain rather than earning its own.
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

/// Startup: build the shared cube mesh/materials (always), and load the creature/GameObject display
/// catalogs off the shared chain (when present). On a catalog failure the resource is simply absent and
/// those entities stay cubes (attach treats both as optional).
fn setup_entities(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    world_assets: Option<ResMut<WorldAssets>>,
) {
    // A person-sized box (2 wide, 4 tall, 2 deep); origin is centered so we lift it by half-height.
    let mesh = meshes.add(Cuboid::new(2.0, 4.0, 2.0));
    // A player without a body model uses a slimmer, shorter block.
    let player_mesh = meshes.add(Cuboid::new(0.8, 2.0, 0.8));
    let mut mat = |r, g, b| {
        materials.add(StandardMaterial {
            base_color: Color::linear_rgb(r, g, b), // GAMMA LANE (0161): raw bytes
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
        // No lock data ⇒ every GameObject reads as lockless (right-click sends USE) — degraded, not broken.
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
        // No LockType data ⇒ lock-bearing GOs fall back to the Interact gear cursor — degraded, not broken.
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
        // No pet tables ⇒ happiness answers its own gate-failure nil (so the icon hides) and the
        // loyalty line is blank; the rest of the paper doll — XP, training points — reads straight
        // off the descriptor and is unaffected. Degraded, not wrong.
        (p, l) => warn!(
            "pet stat tables unavailable, happiness/loyalty stay blank: {:#}",
            p.err().or(l.err()).expect("at least one of the two failed")
        ),
    }
    // The family pair — its own resource, so it degrades independently of the
    // happiness tables above rather than taking them down with it.
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
        // No family tables ⇒ `UnitCreatureFamily("pet")` answers nil, which the reference's own
        // guard turns into a BLANK level line on the pet page (ref `PetPaperDollFrame.lua:68-70`),
        // and the diet tooltip shows nothing at all. That is exactly the state 1057 shipped in —
        // degraded to the faithful fallback, not wrong.
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
    // The glow chain's ItemVisuals join. Absent, weapons draw unadorned.
    match benilla_formats::load_item_visual_catalog(&mut chain) {
        Ok(visuals) => {
            info!("item visual catalog: {} glow rows", visuals.len());
            commands.insert_resource(ItemGlows::new(visuals));
        }
        Err(e) => warn!("item visuals unavailable, weapons never glow: {e:#}"),
    }
    // `SpellItemEnchantment`'s two consumer columns — the enchant half of that glow chain, and
    // the tooltip's enchant line. One load, one resource, both lanes.
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
    // `ItemRandomProperties` — the random-suffix roll's name and its enchant slots 2..6 (decision
    // 1547). Absent, a rolled item reads by its base name and shows no suffix lines.
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

/// For every display id active among the net entities: ensure its [`DisplayModel`] exists (resolve the
/// catalog + request the model handle), and once the handle has loaded, build its spawn parts (the
/// per-submesh material, with creature skin slots filled from the display's variations).
fn update_display_models(
    // `ObjectStore` rides along for the corpses: which cache holds a corpse's model is a
    // descriptor question (`CORPSE_FLAG_BONES`), not a display-id one.
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
    // The UV lane a batch's texture transform is delivered on: an entity batch is
    // seeded AND registered in one call, so a display built here can never be marked-but-frozen.
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    // The bone-pile display cache + the ChrRaces fileStrings its paths are built from (1706).
    mut bones: ResMut<corpse::BonesModels>,
    // The glue-preview want: the glue screens' look's body displayId, so
    // its model builds with no wire entity (the screens run pre-world, where no NetEntity carries it).
    glue_preview: Option<Res<crate::portrait::GluePreview>>,
    char_create: Option<Res<CharCreate>>,
    // The stable pane's want (`SetPetStablePaperdoll` `0x4cb870`): a stabled pet is a row in the
    // server's character-pet cache and a `CreatureDisplayInfo` id — there is no wire entity to
    // carry it, so the booth's own selection is the want.
    stable_booth: Option<Res<crate::portrait::StableBooth>>,
) {
    if !mats.ready() {
        return; // no lighting yet → no materials to build
    }
    let (m2s, wmos) = (&model_assets.0, &model_assets.1);
    // `instance: None` — every display cache here is keyed by display id and shared by every unit
    // wearing it, so these materials belong to the batch. The one population that needs its own
    // (a GameObject whose file-sequence slots bake different loops) takes its clone at spawn,
    // where the host entity exists (`attach::dress`).
    let mut uv = benilla_world::model_render::EntityUvLane {
        reg: &mut uv_reg,
        table: &mut anim_table,
        instance: None,
    };

    // The (kind, display) pairs live in the world this frame — cheap to collect.
    let mut actives: Vec<(EntityKind, u32)> = entities
        .iter()
        // A **bone pile** is skipped here: it carries a perfectly valid display id
        // — the server's corpse→bones conversion copies the field verbatim — and the client
        // deliberately ignores it, so requesting the body it names would load and build a
        // character model nothing will ever render. Its own model is collected below.
        .filter(|(e, store)| {
            !matches!(
                corpse::corpse_model(e, *store),
                Some(corpse::CorpseModel::Bones(..))
            )
        })
        .filter_map(|(e, _)| e.display_id.map(|d| (e.kind, d)))
        .collect();
    // The bone piles want the OTHER cache: their model is keyed (race, sex), so
    // they are collected separately and never enter the display-id want list above — a bone pile's
    // display id is a live field the client deliberately ignores, and building the body it names
    // would load a model nothing renders. Their FLESH siblings need nothing here: a corpse's
    // display id IS a `CreatureDisplayInfo` id, so the `Corpse` arm below resolves it exactly like
    // a unit's.
    let bones_wanted: Vec<(u8, u8)> = entities
        .iter()
        .filter_map(|(e, store)| match corpse::corpse_model(e, store) {
            Some(corpse::CorpseModel::Bones(race, sex)) => Some((race, sex)),
            _ => None,
        })
        .collect();
    // The glue-preview body: a want beside the NetEntity scan, so the
    // character on the glue stage has a model even though nothing streamed it in.
    if let (Some(preview), Some(cc)) = (glue_preview.as_deref(), char_create.as_deref()) {
        if let Some(look) = preview.look {
            let (race, sex) = look.body();
            if let Some(disp) = cc.0.body_display(race, sex) {
                actives.push((EntityKind::Player, disp));
            }
        }
    }
    // The select screen's **pet** — the same kind of wantless want, one display further. It is an
    // ordinary creature display (`Unit`, not `Player`): the enum's pet triple carries a
    // `CreatureDisplayInfo` id, so it resolves down the plain creature chain, skins and all.
    if let Some(preview) = glue_preview.as_deref() {
        if let Some(pet) = preview.look.and_then(|l| l.pet()) {
            actives.push((EntityKind::Unit, pet.display_id));
        }
    }

    // The stable window's selected pet — the same wantless want, and the same `Unit` kind: the
    // creature query answers with a `CreatureDisplayInfo` id, so it resolves down the plain
    // creature chain, skins and all. Asked for whenever the pane names one, live pet or not; a
    // summoned pet's display is already in the scan above, and a repeat is free (the cache is
    // keyed by display).
    if let Some(display) = stable_booth.as_deref().and_then(|b| b.display_id) {
        actives.push((EntityKind::Unit, display));
    }

    for (kind, disp) in actives {
        match kind {
            // Players resolve their body model through the very same creature chain:
            // displayId → CreatureDisplayInfo → CreatureModelData → a `Character\…\…\….mdx` body.
            // …and so does a **corpse** that is not a bone pile: its
            // `CORPSE_FIELD_DISPLAY_ID` is the dead player's own body display, which is the
            // reference's own lookup at `0x5d6759`.
            EntityKind::Unit | EntityKind::Player | EntityKind::Corpse => {
                // Peek through `&` first: `Option<ResMut<T>>::as_deref_mut` marks the resource
                // changed whether or not a byte is written, and `resolve_equipment`'s skip gate
                // reads `Creatures::is_changed()` — so an every-frame `&mut` here re-resolved
                // every rigged unit's equipment every frame (1697 item 1). A display that is
                // present and built asks nothing of the cache.
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
                            false, // gameobject: creatures — no hull collider, no bake variant
                        );
                    }
                }
            }
            EntityKind::GameObject => {
                // The same peek, for the same reason (no gate reads this tick today; the census
                // that finds the next one should not have to look past it).
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
                            true, // gameobject: hull collider + the interior BAKE material variant
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // The bone piles: request each wanted `(race, sex)` skeleton once, then build
    // its parts as the M2 lands — the same two-step as every other display, on its own tiny cache.
    // Character-creation data is the source of the race fileString the path is built from, so a
    // session that could not load it simply shows no skeletons rather than guessing at names.
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
                        false, // gameobject: a prop body — unit lighting, no hull collider
                    );
                }
            }
        }
    }

    // Held-item displays: entries are created by `resolve_equipment`; build each
    // one's parts once its M2 loads. Items are static meshes — no collider, unit lighting.
    // Each of the three path-keyed caches below is `&mut`-borrowed only when it holds an
    // unbuilt entry — the read-side scan is a few hundred `Option` tests, the write-side tick
    // would have read as "changed" on every frame of every run.
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
                        false, // gameobject: held items — unit lighting, no collider
                    );
                }
            }
        }
    }

    // Spell-effect displays (decision 0099 phase 3): entries are created by
    // `spell_fx::resolve_spell_fx`; the same build, keyed by model path instead of display id.
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
                        false, // gameobject: effects — unit lighting, no collider
                    );
                }
            }
        }
    }

    // Item/enchant glow models: path-keyed like the effect cache above, entries
    // created by `resolve_equipment`'s glow resolve.
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
                        false, // gameobject: effects — unit lighting, no collider
                    );
                }
            }
        }
    }
}

/// Compose a unit part's **animated material alpha** into the render-alpha `MeshTag` field — the
/// unit-lane half of the per-batch combine `A = instanceAlpha × colourAlpha × weight`
/// (`0x707680`). The `A ≤ 0` *cull* is already the single `Visibility` authority's
/// (`debug_panel::apply_model_visibility` ANDs `mat_factor > 0`); this is the partial factor, the
/// dimming half, which only a `MeshTag` write can express.
///
/// A fourth alpha writer is exactly what decision 0066's protocol forbids, so this is not one: it
/// writes **only** the alpha field, through [`benilla_world::mesh_tag::with_alpha`] (the probe slot,
/// interior-fog bit, shade byte and highlight bit all ride through), it writes the sampled factor
/// **verbatim** rather than multiplying into what it finds — so re-running it is idempotent, unlike
/// a compounding read-modify-write — and it is ordered after the steady-state owner (the interior
/// classifier) so it re-asserts over that owner's whole-payload reclaim, exactly as
/// `entity_shade::update_ground_shade` does for the shade byte.
///
/// Two owners deliberately keep the channel instead: a live **appear/despawn fade**
/// (`RenderFade`/`PendingAppearFade` — excluded here, and `apply_render_fade` multiplies the factor
/// into its own ramp), and the self-avatar zoom feather, which runs after this and wins on the self
/// body while feathering (its own documented override).
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
    // One breadcrumb per session, the first frame a unit batch actually resolves to the reference's
    // `A <= 0` cull — the machine-readable "this lane is live" signal for a bug whose symptom is
    // geometry that should not be on screen. The live count is in the debug panel's material meter.
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

    /// The reconciler is the *only* thing that makes a wire body visible to the world — no
    /// `WorldUnit` means no ground shade, no WMO room claim, no foam ring, and nothing louder than
    /// a slightly emptier screenshot to say so. So: a body gets one, the viewer gets its marker,
    /// and both track the facts they restate.
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

        // The wire→engine translation 1177 moved here: `benilla-world` no longer knows what a
        // `TYPEID` is, so this is the only place the creature-vs-GameObject question is answered.
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

    /// The viewer marker is reconciled, not stamped once: a `/logout` takes `Embodied` away and
    /// the marker has to go with it, or the next character is feathered as someone else's avatar.
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

    /// One synthetic batch — plain, or a camera-facing one on `bone`.
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

    /// **Every batch the display authored reaches the mirror, split by lane.** This is the same
    /// failure mode decision 1539's imp filed against the select pet: a display-only assembly that
    /// reads one field and drops another renders a creature that is *nearly* right, silently. The
    /// assertion is "everything the cache holds arrives, in the right lane", not a count pinned to
    /// one model.
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

    /// The gate that keeps a half-loaded display out of the booth: `display_mirror` answers `None`
    /// until the parts are built, the same instant [`Creatures::display_rig`] and
    /// [`Creatures::display_anchors`] start answering — so the stand-in never spawns a subject the
    /// booth could frame from fabricated bounds.
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

    /// A body model authoring exactly `slots` (the bone/offset payload is immaterial here).
    fn body(slots: &[u16]) -> BoneAttach {
        BoneAttach {
            points: slots.iter().map(|&s| (s, (0u16, Vec3::ZERO))).collect(),
            markers: HashMap::new(),
        }
    }

    /// `0x6074c0`'s pick and only it: 29 is *preferred*, never required, and never taken by an
    /// unmounted unit. So a character body — which authors both (the shipped HumanMale: 18 at
    /// z 2.2123, 29 at z 1.4029) — moves 18 ↔ 29 across a mount, a creature that authors only 18
    /// (AncientOfLore, Kobold) stays at 18 whether or not it is mounted, and a body authoring
    /// neither anchors nothing: the reference creates the marker and never parents it.
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

    /// The z-bias fallback table `[0x862708]`, verbatim. It is 37 f32 for attachment ids
    /// `0..=0x24`, and the two ids that actually reach it through an animation event are the
    /// emote voice's **17** and a whiffed swing's **1** — so those two are what a drift here
    /// would move, on exactly the models that lack the attachment.
    #[test]
    fn the_z_bias_table_is_the_reference_row_for_ever_id() {
        assert_eq!(ATTACH_Z_BIAS.len(), 0x25, "ids 0..=0x24");
        assert_eq!(ATTACH_Z_BIAS[17], 2.0, "$CSD's fallback lift");
        assert_eq!(ATTACH_Z_BIAS[1], 1.0, "a whiffed $CSS's");
        // The two that are not 1.0-or-1.5, which is what makes a transcription slip visible.
        assert_eq!(ATTACH_Z_BIAS[19], 0.0);
        assert_eq!(ATTACH_Z_BIAS[29], 3.0);
        assert_eq!(ATTACH_Z_BIAS[18], 2.5);
        // Out of range reads as no lift, matching the reference's `0 ≤ id < 0x25` guard.
        assert_eq!(ATTACH_Z_BIAS.get(0x25), None);
    }
}
