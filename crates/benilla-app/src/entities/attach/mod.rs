//! Attaching a visual to a streamed entity — the back half of [`super`].
//!
//! [`super`] resolves + builds a [`DisplayModel`](super::DisplayModel) per display id (shared across
//! every entity of that display); this module gives each net entity its visual once that model has
//! loaded: the submesh children + skeleton/animation infra (creatures + player bodies), the per-player
//! character geoset selection + skin material (the appearance/material resolution
//! lives in [`char_skin`]), particle emitters, GameObject collision, or a colored cube fallback. It
//! reaches the shared types + caches in the parent via `super::`.

use avian3d::prelude::RigidBody;
use benilla_assets::ModelAnimations;
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use benilla_assets::WorldAssets;
use bevy::animation::transition::AnimationTransitions;

use crate::creature_anim::AnimDriver;
use crate::net::{NetEntity, ObjectStore};
use crate::player::CameraPivot;
use crate::target::SelectionRadius;
use benilla_world::interact::WorldObject;
use benilla_world::model_fade::JoinedFade;
use benilla_world::model_render::ModelKind;
use benilla_world::particles;

use super::{
    Characters, Creatures, CubeAssets, DisplayModel, GameObjects, ModelHandle, SkinComposites,
    SkinSections, VisualAttached,
};

mod char_skin;
use char_skin::{build_char_skin_materials, equip_geosets, resolve_char_look, resolve_worn_equip};
mod dress;
mod merge;
use dress::{spawn_group, PartDress};
pub(super) use merge::MergedFormsCache;
mod preview;
pub(crate) use preview::equip_slot;
pub(super) use preview::{build_dressup_preview, build_glue_pet, build_glue_preview};
mod redress;

/// The instruments' view of one body's draw population — what `WOW_DRESS_CENSUS` prints per
/// part (`DRESS_PARTS`): index, merge group, blend, and the material id, so "why nine draws
/// for one body" is a line of output and not a reading of the M2. Lives here because the
/// group and index components are this module's private bookkeeping (0026's dev seam).
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct BodyPartsDesc<'w, 's> {
    parts: Query<
        'w,
        's,
        (
            &'static dress::DressedPart,
            Option<&'static merge::DressedGroup>,
            &'static benilla_world::model_render::ModelPart,
            &'static MeshMaterial3d<benilla_assets::materials::WowModelMaterial>,
        ),
    >,
}

impl BodyPartsDesc<'_, '_> {
    /// One line per drawn part under `unit`, sorted by part index.
    pub(crate) fn describe(&self, unit: Entity, children: &Query<&Children>) -> Vec<String> {
        let mut rows: Vec<(u32, String)> = children
            .iter_descendants(unit)
            .filter_map(|e| self.parts.get(e).ok())
            .map(|(part, group, model, mat)| {
                let group = group
                    .map(|g| format!("{:?}", &g.0[..]))
                    .unwrap_or_else(|| "-".into());
                (
                    part.index,
                    format!(
                        "DRESS_PART #{:<3} group={group:<14} blend={:?} mat={:?}",
                        part.index,
                        model.blend,
                        mat.0.id()
                    ),
                )
            })
            .collect();
        rows.sort_by_key(|(i, _)| *i);
        rows.into_iter().map(|(_, l)| l).collect()
    }
}
pub(super) use redress::redress_player_looks;

/// Set up the skinned instance shared by creatures/players and animated
/// GameObjects: the pose buffer, the palette rig slot, and the global-sequence
/// drive.
///
/// The **sequence clock** — the `AnimationPlayer` + graph + [`ModelAnimations`] + the driver that
/// owns the choice — is NOT set up here; it is [`arm_sequence_clock`], called by the spawn site
/// whether or not this rig was built. Everything in this function costs a palette
/// slot and a skinned twin, and is worth spending only on a model whose bones actually move; the
/// clock is worth arming on every instance, because a sequence drives far more than a pose.
///
/// This lane spawns **no joint entities** — the pose lives in a
/// [`benilla_world::rig_anim::RigPose`] array — and **no anchor entities either** (decision
/// 1355): a bone gets its anchor from `RigPose::anchor_for` the moment something actually
/// consumes it, re-seated from the composed pose each frame it changes. Returns the pose (still
/// in hand — see [`RigBuild`]) + the palette rig slot, or `None` when the model
/// has no inverse bindposes. Slot `0` = the palette table was full: anchors still resolve
/// (emitters/attachments ride them), but parts fall back to the static bind-pose mesh. No
/// animations ⇒ the pose just holds bind pose (Milestone A).
///
/// `skins` is whether anything will actually **skin through this slot** — the same law the item
/// lane's rider states (`equipment::spawn`: "a slot only earns its keep if something will
/// actually SKIN through it"). A body that drew no batch at all (the invisible trigger creature
/// holding a visible weapon) still needs the *pose*, because its attachment
/// points carry what IS drawn, but it has nothing of its own to skin: it takes the slot-less rig
/// [`benilla_world::rig_anim::finalize_rig_worlds`] has supported since 1365, and no `RigStarved`
/// marker either (there is no starvation to heal).
fn setup_skinned_instance(
    commands: &mut Commands,
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    entity: Entity,
    joints_root: Entity,
    d: &DisplayModel,
    skins: bool,
) -> Option<RigBuild> {
    let ibp = d.inverse_bindposes.as_ref()?;
    let nbones = d.skeleton.joints.len();
    // `joints_root` — the rig's model-space frame — is normally `entity` itself; a MOUNTED
    // rider's frame is the seat anchor instead, a conform-tilted model's its
    // conform node, while the `AnimationPlayer`/driver components stay on `entity`. Skinned
    // parts render purely from the palette, so their own parentage is free.
    // No anchor entities are spawned here. A consumer bone gets its anchor the
    // moment something actually consumes it — [`benilla_world::rig_anim::RigPose::anchor_for`],
    // called by the emitter/ribbon/light arms, the billboard dress, held items, the mount seat,
    // spell kits and quest markers — and the position-only readers (the overhead anchor, missile
    // launch points, the bowstring) read `RigPose::posed_point` and never need an entity at all.
    // The eager population this replaced was 96 % never-consumed at the LBRS pin.
    let pose = benilla_world::rig_anim::RigPose::new(joints_root, &d.skeleton);
    // The owned palette rig: the world pass writes this rig's composed frames ×
    // these bindposes into the slot; every skinned part below tags the slot so the vertex stage
    // finds its palette. The on-replace hook frees the slot with the visual teardown.
    let slot = match skins
        .then(|| {
            benilla_world::rig_palette::RigSkin::allocate_bones(
                palettes,
                nbones as u32,
                ibp.clone(),
            )
        })
        .flatten()
    {
        Some(rig) => {
            let slot = rig.slot;
            // The marker's invariant is "currently slot-less": a heal-triggered (or any
            // other) rebuild that lands a slot clears it, or the healer would loop.
            commands
                .entity(entity)
                .insert(rig)
                .remove::<benilla_world::rig_palette::RigStarved>();
            slot
        }
        // Nothing to skin (`skins == false`): the slot-less rig, on purpose and for ever — no
        // starvation marker, or `heal_rig_starved` would rebuild this body every time the table
        // gained headroom, chasing a slot it will never ask for.
        None if !skins => 0,
        None => {
            // Table full (warned): parts render the static bind-pose mesh — but no longer
            // for ever. The marker hands the unit to `heal_rig_starved`,
            // which rebuilds the visual once the table has headroom; without it a mob that
            // streamed in during a full-table window stayed a statue for its whole life.
            commands
                .entity(entity)
                .insert(benilla_world::rig_palette::RigStarved);
            0
        }
    };
    if let Some(anims) = d.animations.as_ref() {
        // Global-sequence bone channels (the eye-blink eyelid scale, resting fidget pulses; a GO's
        // free-running flicker): free-clock loops the per-sequence reader drops, driven on their own clock.
        // Rig-bound (it writes bone slots), unlike the sequence clock — which is armed by
        // [`arm_sequence_clock`] whether or not this instance ended up with a rig.
        if let Some(drive) =
            benilla_world::rig_anim::GlobalSeqDrive::new_rig(&anims.global_bones, nbones)
        {
            commands.entity(entity).insert(drive);
        }
    }
    // The pose buffer travels in the build result, NOT onto the entity yet: the arms of
    // `attach_entity_visuals` below still resolve anchors into it (`RigPose::anchor_for` needs
    // `&mut`), and it lands on the entity in one piece once the build is dressed. The evaluator
    // samples the player state straight into `locals`; with no joint entities and
    // no `AnimationTargetId`s, Bevy's `animate_targets` has nothing of ours to touch.
    Some(RigBuild { pose, slot })
}

/// Arm this instance's **sequence clock**: the `AnimationPlayer` + graph + [`ModelAnimations`] that
/// say which sequence is playing and how far into it, plus the driver that owns the choice
/// ([`crate::go_anim::GoAnim`] for a state GameObject, `AnimDriver` for a unit).
///
/// Separate from the rig on purpose. The two used to be one call — the clock was
/// armed inside [`setup_skinned_instance`], so an instance that wasn't worth skinning got no clock
/// either. But a skinned pose is only ONE of the things a sequence drives: the particle emitters'
/// rate/enable/params tracks read it (`EmitClock::Host`), so do the material alpha/colour/UV loops
/// and the GameObject state arm. 147 GameObject display models animate **only** through those —
/// no keyed bone anywhere — and for them "no rig" silently meant "no clock", i.e. file slot 0 at
/// t = 0 for ever: the Molten Core rune's flames read its *Closed* band, where every emission rate
/// key is 0, and the flame ring's spawn window never opened past its first frame. The clock is a
/// player and a handle; the rig is a palette slot and a skinned twin. Only the second is worth
/// gating on whether bones actually move.
fn arm_sequence_clock(
    commands: &mut Commands,
    entity: Entity,
    anims: &ModelAnimations,
    kind: EntityKind,
    go_state_machine: bool,
) {
    // **Every** GameObject instance gets the loader-idle seed, state machine or not — the
    // reference's `0x70ebd0` tail arms bone 0 the moment the M2 goes LIVE and has exactly two
    // callers, so no M2 instance in the client ever exists with nothing armed. For a door/chest
    // the object-layer arm (`0x5f3930`) lands *after* it and overrides it; seeding first is what
    // stops the one-frame BIND POSE our state GOs used to render on their first displayed frame,
    // before `go_anim` had a chance to run, which reads as a one-frame explosion. The seed
    // is played THROUGH the transitions object so that first arm cleanly fades out of it;
    // playing it bare on the player would leave two clips live at once.
    let mut player = AnimationPlayer::default();
    let mut transitions = AnimationTransitions::new();
    if kind == EntityKind::GameObject {
        // `idle_clip`, NOT `first_seq`: the loader arm's identity answer, taken without the
        // rendering content gate (decision 0936's split). A GameObject whose sequences pose no
        // bone has no `first_seq` at all and used to arm nothing — which is precisely how its
        // emitters ended up reading file slot 0 at t = 0 for ever.
        if let Some(clip) = anims.idle_clip() {
            // Loop iff the sequence says so (`M2Sequence.flags & 1 == 0`) — the kernel's own
            // end-of-band law (`0x714585`): bit0 clear loops on the modulo wrap, bit0 set plays
            // the window and then FREEZES at `end_ms`. An unconditional repeat replayed one-shot
            // idles.
            let active = transitions.play(&mut player, clip.node, std::time::Duration::ZERO);
            if clip.looping {
                active.repeat();
            }
        }
    }
    commands.entity(entity).insert((
        player,
        AnimationGraphHandle(anims.graph.clone()),
        anims.clone(),
    ));
    match kind {
        EntityKind::GameObject if go_state_machine => {
            commands.entity(entity).insert((
                // Cross-fades the open/close transition over the clip's blend-in time,
                // and carries the seed above as the pose the first arm transitions out of.
                transitions,
                crate::go_anim::GoAnim::default(),
            ));
        }
        // A loader-idle GameObject needs no driver and no transitions — the looping player IS the
        // whole animation, and nothing will ever arm over it.
        EntityKind::GameObject => {}
        // A **corpse** gets the clock and the transitions but **no** [`AnimDriver`] (decision
        // 1706): its pose is one settled arm ([`super::corpse::pose_corpses`]), and the driver is
        // the *unit* gait selector — movement flags, sheath ceremony, combat, fidget — none of
        // which an object with no movement block can answer. Enrolling it there would put a
        // corpse in Stand.
        EntityKind::Corpse => {
            commands
                .entity(entity)
                .insert(AnimationTransitions::new())
                .remove::<benilla_world::rig_anim::AnimParked>();
        }
        _ => {
            commands
                .entity(entity)
                .insert((
                    // Cross-fades over each clip's blend-in time, so a gait change eases.
                    AnimationTransitions::new(),
                    AnimDriver::default(),
                ))
                // A (re)built rig is born live: fresh joints spawn pointing at the root, so a
                // stale park marker from the torn-down visual would desync the LOD gate's
                // edge-triggered bookkeeping. It re-parks on its own merits.
                .remove::<benilla_world::rig_anim::AnimParked>();
        }
    }
}

/// A collapsed rig's build result: the pose buffer, still in hand so the build's
/// own arms can resolve anchors into it (anchors spawn on first consumer, via
/// [`benilla_world::rig_anim::RigPose::anchor_for`]), + the palette slot each skinned part tags.
/// The pose lands on the entity at the end of the build.
struct RigBuild {
    pose: benilla_world::rig_anim::RigPose,
    slot: u16,
}

/// Attach a visual to each net entity that doesn't have one yet: its built model (creature / GameObject
/// / player body) as submesh children, or a colored cube fallback. The entity's pose is owned by the
/// net bridge — or, for our own avatar, the player controller — we only add the geometry (and bake
/// per-display scale onto the root). Our own avatar is the same streamed entity and renders here too.
#[allow(clippy::type_complexity)]
pub(super) fn attach_entity_visuals(
    mut commands: Commands,
    pending: Query<
        (
            Entity,
            &NetEntity,
            Option<&super::Equipment>,
            Has<super::equipment::Reattached>,
            Option<&super::mount::MountChild>,
            Option<&super::mount::MountBody>,
            Has<crate::transport::TransportAnchor>,
        ),
        Without<VisualAttached>,
    >,
    // The mount children's build state: a mounted unit's rider waits on its mount
    // child's attach — the seat resolves through the mount's attachment-0 point (`BoneAttach`)
    // into its `RigPose` below; the `NetEntity` is the staleness check (the field moved while
    // the unit was still pending).
    mount_children: super::mount::MountChildren,
    // Already-built rigs' pose buffers — the mount child's, for its seat anchor to resolve on
    // demand. The unit being built here has no `RigPose` component yet (its pose
    // is still in `RigBuild`), so the two never alias.
    mut built_poses: Query<&mut benilla_world::rig_anim::RigPose>,
    // The ItemDisplayInfo catalog (armor region textures). Read-only here; the
    // held-item systems in the same chain hold it mutably in their own turns.
    displays: Option<Res<super::ItemDisplays>>,
    mut transforms: Query<&mut Transform>,
    assets: Res<CubeAssets>,
    creatures: Option<Res<Creatures>>,
    gameobjects: Option<Res<GameObjects>>,
    // The bone-pile body cache — the corpse lane's second model source.
    bones: Res<super::corpse::BonesModels>,
    // Character geoset selection (Milestone B): the customization tables + the entity's
    // decoded appearance, to pick which body geosets a player shows. Absent ⇒ no filtering (every geoset).
    characters: Option<Res<Characters>>,
    // A character body's race/sex + customization for the geoset filter + skin materials: for a player,
    // its decoded descriptor fields ([`ObjectStore`]); a character-model NPC reads its display instead.
    stores: Query<&ObjectStore>,
    // Character skin: the CharSections lookup + the bits to composite +
    // upload the per-appearance body atlas and to load + build the hair material — the shared chain (read
    // the BLPs), the `Image` assets + per-appearance composite cache, the asset server (async hair-BLP
    // load), the `WowModelMaterial` assets + dedup cache, and the light buffer. Nested into one param to
    // stay within Bevy's 16-element system-param tuple limit.
    skin_build: (
        Option<Res<SkinSections>>,
        Option<Res<WorldAssets>>,
        ResMut<Assets<Image>>,
        ResMut<SkinComposites>,
        Res<AssetServer>,
        benilla_world::model_render::M2BatchMaterials,
        // The merged-group forms (`merge`): built on a body's first silhouette, shared after.
        ResMut<Assets<Mesh>>,
        ResMut<merge::MergedFormsCache>,
        // …and the animated-material lane a spawned part may need a material of its OWN on
        // — a GameObject whose file-sequence slots bake different UV or tint
        // loops. Taken by the dressing path itself, because the clone has to exist before the
        // part's interior and fade records are built from it; nested here for this tuple's own
        // stated reason, the 16-param limit.
        ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
        ResMut<benilla_world::doodad_anim::TintAnimMaterials>,
        ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ),
    // The owned skin-palette table: every skinned instance claims a rig slot.
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    // The collider-set stamp (1384): the GameObject hull insert below is the one collider lane
    // outside the streamer's attach queue, so it dates the world itself.
    mut collider_epoch: ResMut<benilla_world::collision::ColliderEpoch>,
    time: Res<Time>,
) {
    let (
        sections,
        world_assets,
        mut images,
        mut skin_composites,
        asset_server,
        mut mats,
        mut meshes,
        mut merged,
        mut uv_reg,
        mut tint_reg,
        mut anim_table,
    ) = skin_build;
    // Arm each entity's appear-fade at the moment its visual attaches (≈ its first-visible moment).
    let now = time.elapsed_secs();
    for (entity, net, equipment, reattached, mount_child, mount_body, anchored) in &pending {
        // A player attaches only once its worn-equipment resolution settles: the
        // template round trips are far faster than the model load, and waiting composites the
        // dressed atlas directly instead of flashing naked. (`None` = the resolver hasn't run yet.)
        if net.kind == EntityKind::Player && !equipment.is_some_and(|e| e.settled) {
            continue;
        }
        // A **corpse** waits on the same component for a different reason: its gear
        // is final on arrival, so this is not a round-trip wait but a *descriptor* wait —
        // `resolve_corpse_equipment` runs one step earlier in this same chain and inserts the
        // component from the store. It matters because the corpse lane has no re-dress: its gear
        // is a death-time snapshot that cannot change, so a body composited before the descriptor
        // landed would stay naked for as long as it lies there.
        if net.kind == EntityKind::Corpse && equipment.is_none() {
            continue;
        }
        // The entity's built display model (if it has one): None ⇒ no display / not a modeled kind.
        //
        // A **corpse** is the one kind whose model cache is not chosen by its display id (decision
        // 1706): `CORPSE_FLAG_BONES` picks between the ordinary creature chain and the
        // `<Race><Sex>DeathSkeleton` props, exactly as the reference's `0x5d6700` does. The fork
        // lives in one function that both this pass and the display build read, so the two can
        // never look in different caches for the same body.
        let dm = match net.kind {
            EntityKind::Corpse => match super::corpse::corpse_model(net, stores.get(entity).ok()) {
                Some(super::corpse::CorpseModel::Bones(race, sex)) => bones.0.get(&(race, sex)),
                Some(super::corpse::CorpseModel::Flesh(disp)) => {
                    creatures.as_deref().and_then(|c| c.models.get(&disp))
                }
                None => None,
            },
            _ => net.display_id.and_then(|disp| match net.kind {
                EntityKind::Unit | EntityKind::Player => {
                    creatures.as_deref().and_then(|c| c.models.get(&disp))
                }
                EntityKind::GameObject => gameobjects.as_deref().and_then(|g| g.models.get(&disp)),
                _ => None,
            }),
        };
        // Worn equipment driving the geoset selection (and, for a player, the region composite): a
        // player from its resolved `Equipment`, a character-model NPC from its display's
        // CreatureDisplayInfoExtra columns. Zeroed (a beast / GameObject / no data) = the naked body.
        let worn = resolve_worn_equip(net, equipment, dm);
        let equip = worn.bodyslots;
        // **Did the display name a model file at all?** An empty `parts` list means two opposite
        // things, and the cube below may only answer one of them: a display that
        // resolved to a model which built zero batches has been *answered* — the model draws
        // nothing — while a display that resolved to no model at all is a gap of ours.
        let named_a_model = dm.is_some_and(DisplayModel::names_a_model);
        // A model still loading (`parts == None`) waits — leave it un-attached and retry next frame
        // rather than flash a cube we'd swap out.
        //
        // **The gate is "did the display name a model", not "did it build a batch"** (decision
        // 1656). A model that named a file and built ZERO batches is a *loaded model instance that
        // draws nothing* — the reference builds its `CM2Model` like any other and the per-batch
        // loop simply has no trip to make — and everything an entity takes from its model but its
        // pixels comes from that instance: the skeleton, the attachment points a held weapon rides,
        // the sequence clock, the PlayerName anchor, the emitters. Keying the whole build on a
        // drawn batch is what left the Naxxramas weapon mobs — an `InvisibleStalker` body holding
        // a visible axe — with no axe and their name plate flat on the floor.
        let model = match dm {
            Some(d) => match &d.parts {
                None => continue,
                Some(parts) => named_a_model.then_some(parts.as_slice()),
            },
            None => None,
        };

        // Invisible interaction-zone GameObjects (the forge, fishing-bobber zone, aura generators, …)
        // carry a *transparent* placeholder M2, and invisible **trigger creatures** an empty or
        // constant-zero-alpha one: the real client's mesh gate is **type-independent** — it draws
        // any loaded model and the per-batch zero-alpha cull skips the transparent geometry
        // (superseding 0023's wrong marker-type gate; zero-alpha cull `0x707b3a`).
        // Our M2 alpha cull already reduces those models to zero submeshes, so `parts` below is
        // EMPTY for them: the spawn loop draws nothing, and `named_a_model` is what keeps the unit
        // arm's debug cube off them (1403).
        if let Some(parts) = model {
            // ── Mounts: a mounted unit is TWO skeletons. The mount is a child
            // entity carrying a plain creature `NetEntity` — this very system builds it like any
            // beast next frame(s) — and the rider's rig then roots under the mount's attachment-0
            // seat joint. Until the mount child has attached, the unit builds nothing (no
            // naked-at-the-ground flash; the model loads dominate the wait anyway). A mount child
            // itself has no `ObjectStore`, so it can never take this branch.
            //
            // This is the **first build** of a unit the field already says is mounted (a
            // born-mounted NPC, a mounted player streaming in). A field change on a unit that is
            // already standing is `mount::reseat_mounts`' job, and it re-seats the standing rig
            // rather than coming back through here — the two share the seat leg below.
            let mount_display = match net.kind {
                EntityKind::Unit | EntityKind::Player => stores
                    .get(entity)
                    .map_or(0, |s| s.0.unit_mount_display_id()),
                _ => 0,
            };
            // Where the rider's root bones parent: the unit entity, or the mounted seat anchor.
            let mut rider_root = entity;
            if mount_display != 0 {
                match super::mount::seat_or_spawn_mount(
                    &mut commands,
                    &mount_children,
                    &mut built_poses,
                    creatures.as_deref(),
                    entity,
                    mount_child.map(|&super::mount::MountChild(c)| c),
                    mount_display,
                    reattached,
                ) {
                    // The mount model is still building (or was just ordered) — wait.
                    super::mount::Seat::Wait => continue,
                    super::mount::Seat::Frame(anchor) => rider_root = anchor,
                    // The reference logs and leaves the body at the unit matrix
                    // (`0x60ce70`'s present-test miss) — `seat_or_spawn_mount` wrote the warning.
                    super::mount::Seat::UnitMatrix => {}
                }
            } else if let Some(&super::mount::MountChild(child)) = mount_child {
                // The field zeroed while this unit was still pending — drop the mount it ordered,
                // or the build below would stand a riderless horse under a dismounted body.
                if let Ok(mut ec) = commands.get_entity(child) {
                    ec.despawn();
                }
                commands.entity(entity).remove::<super::mount::MountChild>();
            }
            // Real model: submesh children inherit the entity's (pose-driven) transform; bake scale onto it.
            let kind = match net.kind {
                EntityKind::GameObject => ModelKind::GameObject,
                _ => ModelKind::Creature,
            };
            let emitters = dm.map(|d| d.emitters.as_slice()).unwrap_or_default();
            let model_lights = dm.map(|d| d.lights.as_slice()).unwrap_or_default();
            // A GameObject's interior-fold reference point (model-local; the anchor transform
            // applies the entity scale when the classifier folds).
            let bake_center = dm.map(|d| d.bake_center_local).unwrap_or(Vec3::ZERO);
            // The dynamic ground-shade root: one light-node state per object, like
            // the reference's `[obj+0xe0]`; every M2 part below (body, held items) reads it from the
            // tree walk. Kind-independent — a unit and a GameObject run the same 2.5/0.5 MCSH chase,
            // which is the byte-shared target law (0814 restored this; 0809 had split it by kind on
            // the strength of a delivery state we do not model). `insert_if_new` so a gear-change
            // re-attach keeps the already-ramped state instead of resetting it (no one-frame
            // lighting pop).
            commands
                .entity(entity)
                .insert_if_new(benilla_world::entity_shade::GroundShade::default());
            // The root's canonical fold reference: held items share the root's interior verdict
            // (one light node per unit — the reference aliases the wearer's collector into each
            // equipped item, `0x718960`), and their classifier fold must reference the BODY's
            // centre, not the carried position. Plain `insert`: a display-id change re-derives it
            // with the new body model.
            commands
                .entity(entity)
                .insert(benilla_world::interior::BodyBakeCenter(bake_center));
            // The light node's ATTACH MODE — the reference's `[node+0x90]` bit 13, written once at
            // node creation from the descriptor TYPEMASK (`0x613e10`/`0x670db0`) and dispatched at
            // `0x6a86d0`: a GameObject attaches by CONTAINMENT (`0x6a8c10`), anchored at the world
            // bounding-box centre; a unit/player by DOWN-RAY (`0x6a8a20`), anchored at the position.
            // The kind can't change under a live display-id swap, but the remove arm keeps the two
            // mutually exclusive by construction rather than by that argument.
            match net.kind {
                EntityKind::GameObject => {
                    commands
                        .entity(entity)
                        .insert(benilla_world::interior::ContainmentAttach);
                }
                _ => {
                    commands
                        .entity(entity)
                        .remove::<benilla_world::interior::ContainmentAttach>();
                }
            }
            // Identity for the mouseover inspector (and, later, hover tooltips / targeting).
            let object = WorldObject {
                kind,
                label: dm.map(|d| display_label(&d.handle)).unwrap_or_default(),
                id: net.display_id.unwrap_or(0),
                detail: format!("emitters: {}", emitters.len()),
            };
            // Skeletal skin: a creature (Unit/Player) — and now any animated
            // GameObject — draws through the skinned-mesh twin and a **per-instance** joint hierarchy
            // whose entities are children of this entity (so they inherit its world pose; at bind pose
            // every joint matrix collapses to that pose, so the model renders exactly where the static
            // mesh did). Truly static props keep the static mesh. `skin` is `Some((joints,
            // inverse_bindposes, palette_slot))` when instanced (decision 0720; slot 0 = palette
            // full, parts fall back to the static mesh).
            // Does this entity run the GameObject open/close state machine (the type dispatch at
            // `0x5f76cc`)? Read once — it picks both the rig flavour below and the clock's
            // driver, and those two must never disagree about the same object.
            let go_state_machine = net.kind == EntityKind::GameObject
                && stores
                    .get(entity)
                    .is_ok_and(|s| crate::go_anim::go_animates(s.0.gameobject_type_id()));
            // Will anything actually SKIN through this instance's palette slot? Every M2 part
            // carries a skinned twin, so for an ordinary body this is simply "it drew something";
            // a body that drew NOTHING (decision 1656's invisible weapon-holder) gets the rig
            // without the slot — see [`setup_skinned_instance`].
            let skins = parts.iter().any(|p| p.skinned_mesh.is_some());
            let mut skin: Option<RigBuild> = match (net.kind, dm) {
                // A creature (or player body) with a real skeleton. The `!is_empty`
                // guard keeps a degenerate boneless model on the static mesh (its skinned twin would
                // carry joint attributes but have no joints to index — out of bounds).
                (EntityKind::Unit | EntityKind::Player, Some(d))
                    if !d.skeleton.joints.is_empty() =>
                {
                    // `rider_root`: the unit itself, or — mounted — the seat anchor under the
                    // mount's attachment-0 joint. The `AnimationPlayer` stays
                    // on the unit entity either way (targets bind by entity, not by path).
                    //
                    // Terrain conform: a flagged model's root bones
                    // parent one level deeper, under a conform node `conform_units` rotates —
                    // wild quadruped and mount child alike. A mounted RIDER never gets one
                    // (`rider_root != entity`): the ref's `0x7106c0` dispatch is on the
                    // mount-PREFERRED model, and the composite tilts through the mount's
                    // node, seat joint included.
                    let mut joints_root = rider_root;
                    if d.terrain_tilt != 0 && rider_root == entity {
                        let node = commands
                            .spawn((
                                super::conform::ConformNode {
                                    // The ground/yaw source: the streamed unit — for a
                                    // mount child, its HOST (the child sits at the unit
                                    // matrix; its own `Transform` is local).
                                    unit: mount_body.map_or(entity, |mb| mb.host),
                                    mode: d.terrain_tilt,
                                },
                                Transform::default(),
                                Visibility::default(),
                            ))
                            .id();
                        commands.entity(entity).add_child(node);
                        joints_root = node;
                    }
                    setup_skinned_instance(
                        &mut commands,
                        &mut palettes,
                        entity,
                        joints_root,
                        d,
                        skins,
                    )
                }
                // A **corpse**. A fresh body is a character model and its held
                // Death pose is a bone pose, so it has to skin or it lies in bind pose — standing
                // up. The `animations` guard is what keeps a **bone pile** off this path: the 16
                // shipped skeletons are fully static two-bone props with no sequence at all, so
                // skinning one could only reproduce the mesh it already renders, at the cost of a
                // scarce palette slot (the same trade decision 0941 made for the poseless GOs).
                (EntityKind::Corpse, Some(d))
                    if !d.skeleton.joints.is_empty() && d.animations.is_some() =>
                {
                    setup_skinned_instance(&mut commands, &mut palettes, entity, entity, d, skins)
                }
                // A GameObject whose model authors a real skeleton + animation draws through the
                // skinned twin like a creature. Two flavours share the rig: a door/button/chest
                // (`go_animates`) runs the open/close state machine off GAMEOBJECT_STATE (decision
                // 0242); ANY other animated GO — a mailbox's wind-swung flags, a banner, a windmill —
                // loops its first sequence as the reference's universal loader-idle seed
                // (`0x70ebd0`: a non-transport CGGameObject animates identically to a placed
                // doodad). The content gate for the non-state flavour is the doodad classifier's:
                // a GO whose first sequence is a constant pose and which has no global sequences
                // (`DoodadAnimTier::Static`) has nothing to loop, so it keeps the static mesh.
                (EntityKind::GameObject, Some(d))
                    if !d.skeleton.joints.is_empty() && d.animations.is_some() =>
                {
                    let state_machine = go_state_machine;
                    let ambient = !matches!(
                        benilla_world::doodad_anim::classify(&d.skeleton, d.animations.as_ref()),
                        benilla_world::doodad_anim::DoodadAnimTier::Static
                    );
                    // A state GO whose model poses NO bone in any sequence gets no rig: skinning
                    // it could only reproduce the bind-pose mesh it already renders, at the cost
                    // of a scarce palette slot (147 display models are this
                    // shape). It still gets the clock below, which is the half it was missing.
                    let poses = d
                        .animations
                        .as_ref()
                        .is_some_and(|a| a.clips.iter().any(|c| c.poses_bones));
                    ((state_machine && poses) || ambient)
                        .then(|| {
                            setup_skinned_instance(
                                &mut commands,
                                &mut palettes,
                                entity,
                                entity,
                                d,
                                skins,
                            )
                        })
                        .flatten()
                }
                _ => None,
            };
            // The instance's SEQUENCE CLOCK — armed for every M2 instance whose model has one,
            // rigged or not. The rig above is a palette slot and a skinned twin,
            // and is worth spending only when bones actually move; the clock is a player and a
            // handle, and everything per-sequence reads it — the emitters' rate/enable/params
            // tracks, the material alpha/colour/UV loops, the GameObject state arm. Bundling the
            // two is what left the boneless-but-animated content (the Molten Core rune's flames,
            // the flame ring's spreading spawn window) reading file slot 0 at t = 0 for ever.
            if matches!(
                net.kind,
                EntityKind::Unit | EntityKind::Player | EntityKind::GameObject | EntityKind::Corpse
            ) {
                if let Some(anims) = dm.and_then(|d| d.animations.as_ref()) {
                    arm_sequence_clock(&mut commands, entity, anims, net.kind, go_state_machine);
                }
            }
            // The bone-riding surface: the model's attachment points + event
            // markers, so held items (and future bone riders) can hang from the hand/hip/back
            // joints. Pure model data — the anchor *entities* live in `RigPose.anchors` and
            // spawn on first consumer.
            if let (Some(_), Some(d)) = (&skin, dm) {
                // The event markers keep the client's first-match scan order: an ident already
                // present wins (character models carry six `$CSD` records — the first is the one
                // `0x7130e0` would return).
                let mut markers = std::collections::HashMap::new();
                for m in &d.markers {
                    markers.entry(m.ident).or_insert((m.bone, m.offset));
                }
                commands.entity(entity).insert(super::BoneAttach {
                    points: d
                        .attachments
                        .iter()
                        .map(|a| (a.id, (a.bone, a.offset)))
                        .collect(),
                    markers,
                });
                // The display-facing counter-twist channels (the strafe body pose): the model's
                // SpineLow/Head key-bones, straight into the pose buffer. Models without either
                // key-bone (beasts, props) get no component — the client's capability gates.
                let nb = d.skeleton.joints.len();
                let in_range = |b: Option<u16>| b.filter(|&i| (i as usize) < nb);
                let (spine, head) = (
                    in_range(d.skeleton.spine_bone),
                    in_range(d.skeleton.head_bone),
                );
                if spine.is_some() || head.is_some() {
                    commands
                        .entity(entity)
                        .insert(crate::creature_anim::BodyTwist::new(spine, head));
                }
            }
            // Character geoset selection (Milestone B): a player body model carries
            // *every* hairstyle / facial-hair / body-option geoset; show only the selected ones. The
            // model (and its `parts`) is shared across all players of this displayId, so the filter is
            // **per-entity** here — from this player's decoded appearance — not baked into the cache.
            // `None` (no look, or the tables unavailable) ⇒ render every part, as before.
            //
            // The character look: a player takes it from the wire, a character-model NPC from its
            // display's CreatureDisplayInfoExtra. Both then drive the same geoset filter
            // + skin/hair materials below; a beast NPC / GameObject has no look and is unaffected.
            let look = resolve_char_look(net, dm, entity, &stores);
            // The worn geoset selectors (the B1–B8 branches): a player's from the
            // resolved equipment display rows; an NPC / naked default otherwise.
            // (The helm's hide-mask row pair, `0x4799a0`: hair/facial/ears tuck under it. For a
            // character-model NPC the helm id is its CreatureDisplayInfoExtra head column.)
            let equip_geosets = equip_geosets(
                displays.as_deref(),
                &equip,
                worn.cloak,
                worn.helm,
                worn.tabard_preview,
            );
            let visible_geosets: Option<Vec<u16>> = look.as_ref().and_then(|l| {
                let cg = characters.as_deref()?;
                Some(cg.0.visible_geosets(
                    l.race,
                    l.sex,
                    l.hair_style,
                    l.facial_hair,
                    &equip_geosets,
                ))
            });
            // Character skin: a character body's body-skin batches (M2
            // type 1) get the body atlas, and its hair batches (type 6) get the hair-mesh texture — both
            // per-appearance over the shared model, so built here (not in the shared model cache);
            // `model_material` then dedups by texture so bodies of one look share them. `(None, None)` ⇒
            // those parts keep their built (untextured) material (no look, or tables/chain absent).
            let char_mats = match look.as_ref() {
                Some(l) => build_char_skin_materials(
                    l,
                    equip,
                    worn.cloak,
                    worn.emblem,
                    worn.tabard_preview,
                    displays.as_deref(),
                    sections.as_deref(),
                    world_assets.as_deref(),
                    parts,
                    &mut images,
                    &mut skin_composites.0,
                    &asset_server,
                    &mut mats,
                ),
                None => (None, None, None, (None, None)),
            };
            // Whether any spawned child armed a `PendingAppearFade` this pass — mirrored onto the unit
            // root below as `UnitAppearFade` so a held item / helm / shoulder that resolves and spawns
            // *later* (`entities::equipment::attach_held_items`, async — a template round trip, a model
            // load) can join the same ramp instead of popping in opaque or racing its own fade from
            // zero. Decision 0032 read as a per-unit property, not a per-mesh-at-attach-time stamp.
            let mut unit_will_fade = false;
            // This unit's **instance slot**, in tag-field form — one value for the whole unit, worn by
            // every part and card below. See the per-part note at the spawn for why it is the
            // instance's identity rather than a skinning detail.
            let inst_slot = skin.as_ref().map_or(0, |rb| rb.slot);
            // The armed idle's **authored** CAaBox — the mouseover picker's
            // volume for a skinned part, NOT a culling volume (skinned entity parts are never
            // frustum-culled; see the `NoFrustumCulling` note at the insert below). The bind-pose
            // box the mesh would otherwise get is only a fair stand-in while the animation keeps
            // the model near rest — the duel flag breaks it: `DuelingFlag.m2` is modelled 9 yards
            // in the air and its Stand translates the root `−9.124` to plant it, so the bind box
            // sits a whole model-height above the drawn geometry. The M2 authors a per-sequence
            // CAaBox for exactly this; for the flag's Stand it is ground-to-tip, which is also
            // what makes the planted flag hoverable where it is actually seen.
            let idle_aabb = dm.and_then(|m| m.animations.as_ref()).and_then(|a| {
                let clip = a.first_seq.and_then(|i| a.clips.get(i))?;
                (clip.bounds_max.cmpgt(clip.bounds_min).all()).then(|| {
                    bevy::camera::primitives::Aabb::from_min_max(clip.bounds_min, clip.bounds_max)
                })
            });
            // This body's **election bound**: standing in a sealed WMO room the
            // reference never submits an outdoor object at all, so the cull needs one whole-object
            // AABB per body, on its root. Recorded here and restated onto `WorldUnit::bound` by
            // `publish_world_units` — the one reconciler that answers every "what is this body to
            // the world" question, exactly as it already does for the collision height.
            //
            // **Not `idle_aabb` above, and that distinction is the whole of it.** `first_seq` is
            // the *rendering* gate — "is this seed worth a skin + player" (0130's content gate) —
            // and it is `None` for every model whose idle does not move its bones off the rest
            // pose. The picker can take that: it falls back per part (`idle_aabb.or(part.aabb)`).
            // The election cannot, because a body with no bound is a body it never decides — so
            // reading the render gate here silently exempted a whole class of creature from the
            // cull, and they kept drawing through the ceiling. `idle_clip` is the same selection
            // asked as an *identity* (`ModelAnimations`' own doc: "conflating the two is the whole
            // defect"), and where even that box is unauthored — a degenerate CAaBox is common in
            // 1.12 art — the model's bind-pose extent is what an unarmed body actually draws.
            let election_bound = dm
                .and_then(|m| m.animations.as_ref())
                .and_then(|a| a.idle_clip())
                .filter(|c| c.bounds_max.cmpgt(c.bounds_min).all())
                .map(|c| bevy::camera::primitives::Aabb::from_min_max(c.bounds_min, c.bounds_max))
                .or_else(|| union_aabb(model.unwrap_or_default().iter().filter_map(|p| p.aabb)));
            if let Some(aabb) = election_bound {
                commands.entity(entity).insert(super::ModelBound(aabb));
            }
            // Everything one part's spawn reads from this unit, gathered once (`attach::dress`) —
            // the same context the gear-change **re-dress** feeds `spawn_part`, so a body built at
            // stream-in and a geoset that appears when a belt is swapped are dressed by one law.
            //
            // Billboard-card bones are the one dress input that needs an anchor ENTITY (the card
            // rides its live joint), so they resolve here — through the same first-consumer
            // resolver as everything else, over the same geoset predicate as the
            // spawn loop below, so a hidden variant's card bone spawns nothing.
            let card_anchors: std::collections::HashMap<u16, Entity> = match skin.as_mut() {
                Some(rb) => parts
                    .iter()
                    .filter(|part| {
                        !visible_geosets
                            .as_ref()
                            .is_some_and(|vis| !vis.contains(&part.geoset_id))
                    })
                    .filter_map(|part| part.billboard.as_ref().map(|b| b.bone))
                    .filter_map(|bone| {
                        rb.pose
                            .anchor_for(&mut commands, entity, bone)
                            .map(|a| (bone, a))
                    })
                    .collect(),
                None => std::collections::HashMap::new(),
            };
            let dress = PartDress {
                unit: entity,
                kind,
                char_mats: &char_mats,
                object: &object,
                inst_slot,
                rigged: skin.is_some(),
                anchors: card_anchors,
                bake_center,
                idle_aabb,
                now,
                // A fresh visual arms the appear-fade on every fade-capable part.
                // A rebuild that is NOT a spawn — a mount transition, a display swap — spawns
                // steady: `Reattached` says the unit was already standing there.
                fade: if reattached {
                    JoinedFade::Steady
                } else {
                    JoinedFade::Pending { since: now }
                },
            };
            // A geoset this character doesn't show (an unselected hair/facial/body variant, or a
            // body region its worn gear replaces) is skipped. The equipment half of that
            // selection is re-run in place on a gear change (`attach::redress`); this is the
            // same predicate, evaluated once at build. The shown batches spawn as material
            // GROUPS (`merge`): one entity per set of batches nothing downstream can tell apart.
            let shows = |part: &super::EntityPart| {
                visible_geosets
                    .as_ref()
                    .is_none_or(|vis| vis.contains(&part.geoset_id))
            };
            let groups = merge::guard_groups(merge::group_parts(parts, shows), parts, &char_mats);
            for group in &groups {
                let forms = merged.forms(parts, group, &mut meshes);
                let part = merge::group_part(parts, group, forms);
                unit_will_fade |= spawn_group(
                    &mut commands,
                    &part,
                    group,
                    &dress,
                    &mut dress::OwnMats {
                        store: mats.materials(),
                        uv: &mut uv_reg,
                        tint: &mut tint_reg,
                        table: &mut anim_table,
                    },
                );
            }
            // Mirror the appear-fade clock onto the unit root (see `unit_will_fade` above): a held item
            // / helm / shoulder attaching later reads this to join the same ramp
            // (`entities::equipment::attach_held_items`). A `Reattached` rebuild — a mount
            // transition, a display swap — abandons any in-flight fade outright, matching the body
            // parts it just rebuilt steady, so anything spawned for that same rebuild is steady too.
            // (A GEAR change no longer reaches here at all: it re-dresses in place, and the parts it
            // spawns join this clock instead of clearing it — `attach::redress`.)
            if unit_will_fade {
                commands
                    .entity(entity)
                    .insert(benilla_world::model_fade::UnitAppearFade::Pending { since: now });
            } else if reattached {
                commands
                    .entity(entity)
                    .remove::<benilla_world::model_fade::UnitAppearFade>();
            }
            // Final size = the server's per-object scale (`OBJECT_FIELD_SCALE_X`) alone. The server
            // already folds the unit's DBC scale (`CreatureModelData.modelScale ×
            // CreatureDisplayInfo.scale`, or an explicit per-spawn override) into this field, and the
            // real client renders units at the field alone (`0x613ef0`, vmangos
            // `Unit::GetScaleForDisplayId`). Multiplying our own DBC scale on top
            // double-applies it — `native²`, worst for the sub-1.0 starting-zone scales. A GameObject's
            // display scale was always 1.0, so this is unchanged for it.
            let placement = if let Ok(mut t) = transforms.get_mut(entity) {
                t.scale = Vec3::splat(net.scale);
                *t
            } else {
                Transform::default()
            };
            // The equipment this visual was dressed with: `redress_player_looks`
            // diffs it against the live resolution and re-dresses the standing visual on a change.
            if let (EntityKind::Player, Some(e)) = (net.kind, equipment) {
                commands
                    .entity(entity)
                    .insert(super::equipment::AppliedEquipment(*e));
            }
            // Camera framing-pivot height (model-derived, pre-scale): the self-avatar reads it in the
            // camera controller to target ~neck height instead of a fixed offset (harmless on NPCs).
            commands.entity(entity).insert(CameraPivot {
                height_local: dm.map(|d| d.pivot_height_local).unwrap_or(0.0),
                // …and how far that pivot drops while this body swims — the reference's third
                // preset `cam+0x124`, stored as the delta it is built from (`0x50ccf6`). `0.0` for
                // anything with no Swim sequence, so the two presets coincide.
                swim_drop_local: dm.map(|d| d.swim_pivot_drop_local).unwrap_or(0.0),
            });
            // The overhead-anchor fallback input (combat text over a model with no PlayerName
            // attachment — `0x608640`'s defensive branch).
            commands.entity(entity).insert(super::OverheadFallback(
                dm.map(|d| d.bbox_z_local).unwrap_or(0.0),
            ));
            // The chat bubble's own anchor height — the Stand box's Z, a file constant, NOT the
            // posed attachment the overhead name above rides (1406).
            commands.entity(entity).insert(super::StandBoxHeight(
                dm.map(|d| d.stand_box_z_local).unwrap_or(0.0),
            ));
            // Selection-ring radius (model-local sphere radius, pre-scale) — the targeting ring reads it
            // × the unit's scale (harmless on non-unit models, which are never ringed).
            commands.entity(entity).insert(SelectionRadius(
                dm.map(|d| d.ground_radius_local).unwrap_or(0.0),
            ));
            // Particle emitters (flames/glows) — spawned per entity, despawning with it. A skinned
            // creature's emitter rides its host bone's joint with the model-space origin rebased
            // into the bone frame (`position − pivot`), exactly like a doodad emitter (0130 phase 4,
            // same rig identity) — the kobold's candle flame follows the head through the crouch
            // instead of floating at the rest-pose height. GameObjects/boneless models keep
            // whole-entity follow (no joints; their bones hold rest pose anyway).
            {
                for em in emitters {
                    let owner = skin
                        .as_mut()
                        .and_then(|rb| rb.pose.anchor_for(&mut commands, entity, em.def.bone))
                        .map_or((entity, [0.0; 3]), |j| (j, em.bone_pivot));
                    particles::spawn_emitter(
                        &mut commands,
                        em,
                        placement,
                        particles::EmitterFrames {
                            owner: Some(owner),
                            // The cloud SORTS at the unit; bones compose births only. Since 1585
                            // a world-mode store is frozen at birth, so neither can move a live
                            // particle. (`[model+0x17c]` is the TRANSPORT matrix — 1591 — so it is
                            // 0 for a unit on foot and this is not the attached/unattached split
                            // it used to claim to be.)
                            anchor: Some(entity),
                            // The unit's model going away IS this emitter's model going away
                            // (stream-out, a visual rebuild) — free the pool with it.
                            on_owner_loss: particles::OwnerLoss::Free,
                            // The unit IS the model instance here, so its own render alpha
                            // (appear ramp, stream-out ramp, the self-avatar feather) multiplies
                            // its clouds.
                            alpha: Some(entity),
                            // …and the entity IS the light node: one node per object, created
                            // from its TYPEID and filled every frame whether or not a mesh batch
                            // consumes it. A LIT emitter is a consumer of that node exactly as a
                            // lit batch is, and on the Onyxia lava traps it is the ONLY one
                            // (`crate::entities` → `benilla_world::interior::EmitterLitBy`).
                            light_node: Some(entity),
                        },
                        // The emitters' rate/enabled read this instance's PLAYING sequence — a
                        // unit's or GameObject's `AnimationPlayer` on the root. A quest object's
                        // explosion is authored inside its one-shot clips with an OFF window at
                        // idle; a creature's death-only smoke is the same shape.
                        particles::EmitClock::Host(entity),
                    );
                }
            }
            // The model's own M2 point lights — a fire elemental's glow, a lit
            // GameObject brazier. Same host-bone ride as the emitters, for the same reason: the
            // reference re-registers each light at its LIVE bone position every frame. (The far more
            // common carried light is the held torch — that one spawns on the item model, in
            // `equipment`.) The light bones resolve BEFORE the call: the closure can't borrow
            // `commands` while `spawn_carried_lights` holds it.
            let light_anchors: std::collections::HashMap<i16, Entity> = model_lights
                .iter()
                .filter(|l| l.def.casts())
                .filter_map(|l| {
                    let bone = u16::try_from(l.def.bone).ok()?;
                    let rb = skin.as_mut()?;
                    rb.pose
                        .anchor_for(&mut commands, entity, bone)
                        .map(|a| (l.def.bone, a))
                })
                .collect();
            super::spawn_carried_lights(&mut commands, model_lights, entity, |bone| {
                light_anchors.get(&bone).copied()
            });
            // Ribbon trails (wisp streamers, trailing quest-object crystals) — the same host-bone
            // ride as the emitters; the trail self-despawns when its owner joint/entity goes.
            {
                for rb in dm.map(|d| d.ribbons.as_slice()).unwrap_or_default() {
                    let (owner, use_pivot) = skin
                        .as_mut()
                        .and_then(|build| build.pose.anchor_for(&mut commands, entity, rb.def.bone))
                        .map_or((entity, false), |j| (j, true));
                    // The `+0xc0` enable gate reads THIS instance's playing sequence, live —
                    // a GameObject flips state under its own trails (a trap springs, a door
                    // opens), so there is no spawn-time answer. Passing `None` here (the old
                    // "body models don't author a gate" shortcut, true of creatures and false of
                    // GameObjects) drew every trail a model authored in every state: the Frost
                    // Trap's twelve trigger-only streamers became a permanent spinning column
                    // over the placed trap.
                    benilla_world::ribbons::spawn_ribbon(
                        &mut commands,
                        rb,
                        owner,
                        use_pivot,
                        placement.scale.max_element(),
                        benilla_world::ribbons::RibbonSeq::Host(entity),
                        // The unit's own render alpha gates its trail.
                        Some(entity),
                        // No fade sphere: a streamed unit is not a placed model — its
                        // population is bounded by server visibility, and its trail by the
                        // unit's own render alpha one line up.
                        None,
                    );
                }
            }
            // The pose buffer lands last: every build arm above has resolved its
            // anchors into it, so it arrives on the entity in one piece — the evaluator, the
            // compose pass and every later `anchor_for` (a weapon equipped in combat, a spell
            // kit) find the same buffer the build populated.
            if let Some(rb) = skin {
                commands.entity(entity).insert(rb.pose);
            }
            // Static collision for solid GameObjects (chests, mining veins, doors…): the model-local
            // collider baked at build time rides the entity's pose, so player + camera collide with it.
            // Hull-less GameObjects (herbs, small props) carry none — collide-iff-hull. GameObjects only;
            // creatures use unit-collision, not modeled here. An anchored transport (boat/lift) is
            // Kinematic — its body moves every frame, and a Static insert here would silently
            // overwrite the arm's label when the asset finished loading after the arm ran.
            if matches!(net.kind, EntityKind::GameObject) {
                if let Some(col) = dm.and_then(|d| d.collider.clone()) {
                    let body = if anchored {
                        RigidBody::Kinematic
                    } else {
                        RigidBody::Static
                    };
                    commands.entity(entity).insert((body, col));
                    // This lane bypasses the streamer's attach queue entirely, so it stamps
                    // the collider set itself — otherwise a creature standing where a GameObject
                    // hull lands frames later keeps the answer it took before there was one.
                    collider_epoch.bump();
                }
            }
        } else {
            // Cube fallback: other players (cyan, slim block) and NPCs (red, person-box) whose display
            // named **no model to load** — the debug signal for a gap of ours. The cube origin is
            // centered, so a child offset lifts it onto the ground.
            //
            // A display that DID name a model renders exactly what that model built — including
            // nothing (bug B13). The reference's mesh gate is type-independent: it
            // draws any loaded model and the per-batch zero-alpha cull skips what is transparent
            // (zero-alpha cull `0x707b3a`), so a model with no surviving batch
            // submits no geometry and the unit is invisible. Byte-exact, folded back at 1407: the
            // batch loop's TRIP COUNT is the selected ModelView's texUnit count (`0x707a72`), so a
            // view with none skips the whole loop — and the unit lane reaches that same loop, on
            // the same singleton `CM2Scene`, through the same `0x613e10` as every other typeId.
            // That is the whole of how a **trigger creature** hides —
            // `Creature\InvisibleStalker\InvisibleStalker.m2` ships with
            // `nVertices == 0`, and its `NoName` sibling's one batch carries a constant-zero
            // transparency track — no unit flag is consumed anywhere. Cubing them put a black slab
            // over 154 vmangos templates on 8 displays (the Scarab Wall's Anachronos trigger,
            // Yojamba's Zandalarian Event Generator). It was only ever the *unit* arm that got this
            // wrong; the GameObject arm below has drawn nothing since 0024.
            let fallback = match net.kind {
                _ if named_a_model => None,
                EntityKind::Player => {
                    Some((assets.player_mat.clone(), assets.player_mesh.clone(), 1.0))
                }
                EntityKind::Unit => Some((assets.npc_mat.clone(), assets.mesh.clone(), 2.0)),
                EntityKind::GameObject => {
                    debug!(
                        "gameobject (display {:?}) has no usable model — not rendering",
                        net.display_id
                    );
                    None
                }
                // A DynamicObject is deliberately invisible as an *object* — its look is the
                // spell's area effect (the dest-anchored visual lane), never a fallback cube.
                //
                // A **corpse** takes no cube either, and for the GameObject arm's
                // reason rather than the DynamicObject's: it reached here only because its display
                // named no model — a `CORPSE_FIELD_DISPLAY_ID` absent from `CreatureDisplayInfo`,
                // or a bone pile whose race has no shipped skeleton. Both are the reference's own
                // null-row legs, which draw nothing (`0x5d6700` returns 0 and the model never
                // loads); a body-sized red box on the ground would read as a bug in itself.
                EntityKind::Corpse => {
                    debug!(
                        "corpse (display {:?}) has no usable model — not rendering",
                        net.display_id
                    );
                    None
                }
                EntityKind::DynamicObject | EntityKind::Other => None,
            };
            if let Some((material, mesh, lift)) = fallback {
                // Tag the cube as a pickable unit too (kind `Creature` covers units + player bodies), so
                // a model-less NPC / other player can still be inspected and, crucially, targeted.
                let object = WorldObject {
                    kind: ModelKind::Creature,
                    label: format!("{:?} (no model)", net.kind),
                    id: net.display_id.unwrap_or(0),
                    detail: String::new(),
                };
                commands.entity(entity).with_children(|parent| {
                    parent.spawn((
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        Transform::from_translation(Vec3::Y * lift),
                        object,
                        // …and into the unit pick's marker-filtered fallback population like any
                        // dressed creature part.
                        benilla_world::interact::CreaturePickPart,
                        // The cube has no resident `RenderSubmesh` to cast against — its `Aabb` IS
                        // its shape, which it must SAY: the picker requires pick
                        // geometry rather than inferring a box from its absence.
                        benilla_world::interact::PickBox,
                        // What makes "we are standing a cube here" a number a probe can read
                        // (`WOW_UNIT_VISUALS`) rather than something only an eye finds — 1403.
                        super::FallbackCube,
                    ));
                });
            }
        }
        // The mount this visual was built with (the `AppliedEquipment` pattern):
        // `refresh_mounts` diffs it against the live field and rebuilds on any transition. Written
        // on the cube fallback too (same read, no seat), so a model-less unit can never churn.
        if matches!(net.kind, EntityKind::Unit | EntityKind::Player) {
            let applied = stores
                .get(entity)
                .map_or(0, |s| s.0.unit_mount_display_id());
            commands
                .entity(entity)
                .insert(super::mount::AppliedMount(applied));
        }
        commands
            .entity(entity)
            .insert(VisualAttached)
            // The display this visual was built with (the same pattern):
            // `refresh_live_display` diffs it against the live descriptor and rebuilds on a
            // change (druid form, GM morph). Stamped on the cube fallback too, so a model-less
            // unit can never churn.
            .insert(super::live_display::AppliedDisplay(net.display_id))
            .remove::<super::equipment::Reattached>();
    }
}

/// Spawn a rig's joint-entity hierarchy under `root`: one entity per bone
/// carrying its rest-local translation, parented per the skeleton — root bones under `root` so
/// they inherit the entity's world pose, others under their parent joint. Returns the joints in
/// bone order, so a vertex's joint index maps straight in and every submesh's palette rig shares
/// this one set. **The doodad/effect/booth lane only**: a streamed unit's rig is
/// the joint-less [`benilla_world::rig_anim::RigPose`] buffer instead — this hierarchy remains for
/// the Bevy-graph-driven hosts. `holder` is the rig root that carries (or will carry) the
/// [`benilla_world::rig_palette::RigSkin`] — every joint marks it with a `RigJoint`, which is what the
/// palette change-sweep iterates.
/// The enclosing box of a model's parts — its bind-pose extent, in model space.
///
/// The election bound's fallback for a body whose armed idle authors no CAaBox.
/// Every part of one model shares that space, so their union is the model, and for a body that is
/// not armed at all it is exactly what gets drawn. A bind box can be a poor bound for a body whose
/// idle moves it far (0637's duel flag, modelled 9 yd up and planted by its Stand) — which is why
/// the authored box is preferred whenever there is one, and why this is a fallback rather than the
/// rule. Over-large is the safe direction here: it can only admit, never hide.
fn union_aabb(
    boxes: impl Iterator<Item = bevy::camera::primitives::Aabb>,
) -> Option<bevy::camera::primitives::Aabb> {
    let mut boxes = boxes;
    let first = boxes.next()?;
    let (mut min, mut max) = (first.min(), first.max());
    for b in boxes {
        min = min.min(b.min());
        max = max.max(b.max());
    }
    Some(bevy::camera::primitives::Aabb::from_min_max(
        Vec3::from(min),
        Vec3::from(max),
    ))
}

/// A display model's source path as a readable inspector label (the asset path, sans `mpq://` source).
/// Empty for the model-less variant or a path-less handle.
fn display_label(handle: &ModelHandle) -> String {
    let path = match handle {
        ModelHandle::M2(h) => h.path(),
        ModelHandle::Wmo(h) => h.path(),
        ModelHandle::None => None,
    };
    path.map(|p| p.path().to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_assets::{ModelJoint, ModelSkeleton};
    use bevy::ecs::world::CommandQueue;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

    /// A one-bone rest skeleton + its bindposes — the smallest rig `setup_skinned_instance` will
    /// build, so the test is about the slot policy and nothing else.
    fn one_bone(ibps: &mut Assets<SkinnedMeshInverseBindposes>) -> DisplayModel {
        DisplayModel {
            skeleton: ModelSkeleton {
                joints: vec![ModelJoint {
                    parent: -1,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: None,
                }],
                spine_bone: None,
                head_bone: None,
            },
            inverse_bindposes: Some(
                ibps.add(SkinnedMeshInverseBindposes::from(vec![Mat4::IDENTITY])),
            ),
            ..super::super::display::empty_shell()
        }
    }

    /// **A body with nothing to skin gets the rig without the palette slot**.
    ///
    /// The invisible trigger creature holding a visible weapon is a complete model instance — it
    /// needs the pose, because its attachment points carry what IS drawn — and has no geometry of
    /// its own for a palette slot to serve. It must also take **no `RigStarved`**: that marker
    /// means "currently slot-less and wants one", and `heal_rig_starved` would rebuild this body
    /// every time the table gained headroom, for ever, chasing a slot it never asks for.
    #[test]
    fn a_body_that_skins_nothing_takes_the_rig_without_a_slot() {
        use benilla_world::rig_palette::{RigPalettes, RigSkin, RigStarved};

        let mut app = App::new();
        app.init_resource::<RigPalettes>()
            .init_resource::<Assets<SkinnedMeshInverseBindposes>>();
        let mut ibps = app
            .world_mut()
            .remove_resource::<Assets<SkinnedMeshInverseBindposes>>()
            .unwrap();
        let d = one_bone(&mut ibps);

        for (skins, want_slot) in [(false, false), (true, true)] {
            let mut world = std::mem::take(app.world_mut());
            let entity = world.spawn_empty().id();
            let mut palettes = world.remove_resource::<RigPalettes>().unwrap();
            let build = {
                let mut queue = CommandQueue::default();
                let mut commands = Commands::new(&mut queue, &world);
                let build =
                    setup_skinned_instance(&mut commands, &mut palettes, entity, entity, &d, skins);
                queue.apply(&mut world);
                build
            };
            world.insert_resource(palettes);
            assert!(
                build.is_some(),
                "the pose is built either way (skins={skins})"
            );
            let e = world.entity(entity);
            assert_eq!(
                e.contains::<RigSkin>(),
                want_slot,
                "skins={skins} ⇒ palette slot {want_slot}"
            );
            assert!(
                !e.contains::<RigStarved>(),
                "skins={skins}: a table with room never starves — and a body that skins nothing \
                 must never ask to be healed"
            );
            *app.world_mut() = world;
        }
    }
}
