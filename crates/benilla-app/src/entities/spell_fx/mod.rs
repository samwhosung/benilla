//! Spell-visual **effect models** (decision 0099 phase 3): the attach-point `.mdx` glows a
//! spell-visual kit hangs on a casting unit — Fireball's burning hands, a heal's golden sparkle.
//! The **lootable-corpse sparkle** rides the same plumbing (the client hangs it on the same
//! `Effect_C` node type): `creature_anim::spell_visual::arm_loot_fx` writes its Begin/Reap edges
//! under a reserved key.
//!
//! The router (`crate::creature_anim::spell_visual::route_cast_visuals`) resolves each cast edge's
//! kit to `(M2 attachment id, model path)` pairs and emits [`SpellKitFx`] edges; this module owns
//! the render side: a path-keyed [`DisplayModel`] cache (the held-items pattern — entries created
//! here, parts built by `super::update_display_models` once the M2 loads), and per-unit effect
//! instances spawned under the unit's [`BoneAttach`] joints so they ride the animating bone —
//! exactly the client's `CEffect` bone attach (`0x620be0`), re-resolved per frame off the live
//! bone matrix (riding the joint entity gives us the same for free).
//!
//! Lifetime is the client's stage policy (decision 0107 verdict 2): a **persistent** instance
//! (precast/channel) lives until its spell-id-keyed [`SpellKitFx::Reap`] (the client's
//! `0x614150`); a **self-terminating** one (cast release, kit push) despawns after one pass of
//! its model's sequence 0 — the stage-0/1 completion callback's clock, which runs whether or not
//! the sequence moves a bone (the eat/drink tankard is a 6.667 s sequence with zero bone keys,
//! so it outlives the ~5 s kit-resend cadence and the jug is held continuously).
//!
//! Every spawn first runs the **same-slot replace** ([`replace_same_slot`]): the
//! reference's `0x6208e0`, which destroys any live instance of the same `SpellVisualEffectName`
//! record at the same attach tag. It is what keeps a busy fight's effect count flat in the number
//! of attackers, and it is also what swaps the tankard out on each resend.
//!
//! **The attach cascade below is mis-attributed and is a known divergence** (named in 2057, not
//! yet fixed): `tag → 0xf → 0x13 → the unit's base` is `CMissile`'s (`0x61ceb0`,
//! `Missile_C.cpp`), not `AddEffect`'s: `0x61fdd0` has no attachment test at all, so on a model
//! lacking the tag the reference drops the effect permanently and invisibly where we relocate it.
//!
//! Effect models run their **bone rigs** ([`arm_effect_rig`] — the birth clip + global sequences
//! pose the joints that meshes skin to and emitters/ribbons/cards ride), advance them through the
//! authored `Stand` → `Hold` → `Decay` lifecycle ([`lifecycle`], which is where the Ice Barrier
//! shield's pulse comes from), and run their **material
//! animation**: each part's colour-alpha × transparency-weight loops sample per instance on the
//! attach clock (a [`MatAnim`](benilla_world::doodad_anim::MatAnim) that owns the part's render-alpha
//! tag — Battle Shout's staggered crescent pulses), an animated M2Color RGB ticks a
//! **per-instance material clone**'s tint uniform ([`FxTintAnims`] — the white-hot flash cooling
//! to red; per instance because one cast = one phase, unlike the doodad lane's shared-clock
//! loops in [`benilla_world::doodad_anim::TintAnimMaterials`]), and a **texture transform** scrolls
//! that same clone's UVs off the instance's own clip — translation, rotation and scale, through
//! [`register_fx_uv`](benilla_world::doodad_anim::register_fx_uv)'s effect lane (
//! the druid's claw trail is nothing but that scroll, so until it landed the trail did not draw
//! at all).
//!
//! Effect instances **fire their model's event track** ([`fire_fx_anim_events`]):
//! the playing clip's `$SND`-family keyframes emit the same [`AnimSoundEvent`] stream creatures
//! emit (spatialized at the host unit), so an effect whose sound lives in its own M2 — the
//! level-up pillar's `$SND(888)` at t=0.033s — rings without any code-side kit. (The ding's
//! WATCHER — the `UNIT_FIELD_LEVEL` edge that spawns the pillar — lives with its lootable-edge
//! sibling in `creature_anim::spell_visual::arm_level_up_fx`, the SpellKitFx writers' home.)
//!
//! Approximations, named: a ground-anchored effect's flat quad parts carry no UV offset — their
//! draw identity rides a `GroundFxDecal` record rather than a `WowModelMaterial`, and no
//! transform-animating effect model authors one, so the population is empty (`benilla-extract
//! fxuvscan` is where that stays measured rather than remembered); the span-based
//! self-termination stands in for the client's model-event completion callback; and the kit sound
//! keeps playing unconditionally where the client gates it on no-visual-attached.

mod lifecycle;

use std::collections::HashMap;

use benilla_assets::bone_target_id;
use bevy::animation::AnimatedBy;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::creature_anim::spell_visual::FxSlot;
use crate::creature_anim::{scan_events, AnimSoundEvent, FxClass, FxStage, SpellKitFx};
use benilla_assets::m2_url;
use benilla_assets::materials::WowModelMaterial;
use benilla_world::model_render::{ModelKind, ModelPart};
use benilla_world::particles;
use benilla_world::vis_chain::VisChainOnly;

use super::{BoneAttach, DisplayModel, EntityPart, ModelHandle};
pub(super) use lifecycle::advance_fx_anim;
use lifecycle::{decay_span, FxAnimLife, FxDecay};

/// The client's attach fallback cascade when a model lacks the requested point
/// (`0x61ceb0`/`0x61fae0`): retry `0xf`, then `0x13`, then the unit's base position.
const ATTACH_FALLBACKS: [u16; 2] = [0xf, 0x13];

/// The self-termination span when an effect model has no sequence table at all (the cube
/// fallback and kin) — long enough to read as a flash, short enough never to linger. Shared
/// with the dest-anchored lane's one-shots ([`super::dest_fx`]) — same completion-callback
/// stand-in.
pub(crate) const FALLBACK_SPAN: f32 = 1.0;

/// How long a self-terminating instance may wait unspawned (model/skeleton never loading — the
/// cube-fallback caster) before it is dropped instead of pending forever.
const PENDING_TIMEOUT: f32 = 10.0;

/// The effect-model cache: model path → its [`DisplayModel`] (the held-items pattern — the shell
/// is created here on first use; `super::update_display_models` builds `parts` once the asset
/// loads, shared by every instance of that path).
#[derive(Resource, Default)]
pub(crate) struct SpellFx {
    pub(crate) models: HashMap<String, DisplayModel>,
}

/// Get-or-insert this path's cache entry, starting its M2 load on the spot — **the only way any
/// site may reach [`SpellFx::models`]**, read or write.
///
/// [`super::evict_display_caches`] clears every display cache on a `MapChange`, and its doc states
/// the contract that makes that safe: *"these caches are get-or-insert at every use site, so a
/// cleared entry rebuilds on the next spawn that wants it."* This cache was the one that broke it.
/// Its three consumers looked entries up with a bare `get` and each carried a comment calling the
/// miss unreachable — "created with the instance", "the spawn created the cache entry" — because
/// the entry *is* created with the instance, one system earlier. What none of them could see is
/// that an eviction lands **between** the two, and the entry is never re-created: only a fresh
/// `SpellKitFx::Begin` inserts, and an aura already sitting in its slots never emits another.
///
/// The map flip that does it is the ordinary one: `world_map::announce_map_change` fires on the
/// login `0 → 1`, ~0.4–1.7 s into every session, which is exactly when a body that entered the
/// world already carrying an aura is arming that aura's state kit. A **persistent** instance
/// stranded there is silent and permanent — the `PENDING_TIMEOUT` reaper is gated
/// `!inst.persistent`, so it is never spawned, never expired, and never traced. Measured at 9 runs
/// in 10 (bug: the stun swirl not showing).
pub(super) fn ensure_model(fx: &mut SpellFx, asset_server: &AssetServer, path: &str) {
    fx.models
        .entry(path.to_string())
        .or_insert_with(|| DisplayModel {
            handle: ModelHandle::M2(asset_server.load(m2_url(path))),
            ..super::empty_shell()
        });
}

/// The live **per-instance tint clones**: an effect part whose M2Color RGB animates gets its own
/// material clone at attach (one cast = one phase — the doodad lane's shared-clock registry
/// [`benilla_world::doodad_anim::TintAnimMaterials`] would run every Battle Shout on one global pulse),
/// keyed by the clone's asset id → `(RGB loop, attach-time clock origin)`. An entry drops with
/// its material asset — the instance root's despawn releases the only strong handle.
#[derive(Resource, Default)]
pub(crate) struct FxTintAnims(
    HashMap<
        bevy::asset::AssetId<WowModelMaterial>,
        (std::sync::Arc<benilla_formats::RgbAnim>, f32),
    >,
);

/// Re-sample every live fx clone's tint on its instance clock. Deliberately NOT capture-gated
/// (unlike the doodad lane's shared-clock ticks): the `fxview` instrument ages an effect inside a
/// capture, and golden scenarios spawn no effects, so determinism is unaffected.
pub(crate) fn tick_fx_tint(
    time: Res<Time>,
    mut reg: ResMut<FxTintAnims>,
    mut materials: ResMut<Assets<WowModelMaterial>>,
) {
    if reg.0.is_empty() {
        return;
    }
    let now = time.elapsed_secs();
    reg.0.retain(|id, (anim, origin)| {
        let Some(mat) = materials.get_mut(*id) else {
            return false; // the instance despawned — its clone unloaded with it
        };
        let rgb = anim.sample(now - *origin);
        mat.extension.tint = Vec4::new(rgb[0], rgb[1], rgb[2], 1.0);
        true
    });
}

/// The material side of an effect attach, in one parameter: the store a part's clone is made
/// from, and the three places that clone can be registered for per-instance animation — the tint
/// loop ([`FxTintAnims`]), the UV scroll ([`UvAnimMaterials`]' effect lane,
/// decision 2282) and the delta table both scrolls write their rows into. They always travel
/// together, and a caller that threaded some of them silently dropped a channel.
pub(crate) struct FxMaterials<'a> {
    pub(crate) store: &'a mut Assets<WowModelMaterial>,
    pub(crate) tint: &'a mut FxTintAnims,
    pub(crate) uv: &'a mut benilla_world::doodad_anim::UvAnimMaterials,
    pub(crate) table: &'a mut benilla_world::mat_anim_table::MatAnimTable,
}

/// Resolve one part's material for a NEW effect instance. The shared handle, unless a channel has
/// to run **per instance** — an animated M2Color RGB or a texture transform
/// — and then a clone of it, seeded at each live loop's first key and registered
/// for the per-frame ticks. `host` is the instance root, whose `AnimationPlayer` the UV lane reads
/// its clip off (the sequence source `MatAnim::following_host` takes beside it).
fn fx_part_material(
    part: &EntityPart,
    now: f32,
    host: Entity,
    mats: &mut FxMaterials,
) -> Handle<WowModelMaterial> {
    let loops = part.uv_loops();
    if part.rgb_anim.is_none() && !loops.any() {
        return part.material.clone();
    }
    // The shared steady may still be parked (`model_render::lazy`): this copy is made before
    // anything binds it.
    benilla_world::model_render::lazy::realize(mats.store, part.material.id());
    let Some(mut mat) = mats.store.get(part.material.id()).cloned() else {
        return part.material.clone(); // shared material not built yet — parts were checked ready
    };
    if let Some(anim) = &part.rgb_anim {
        let t0 = anim.sample(0.0);
        mat.extension.tint = Vec4::new(t0[0], t0[1], t0[2], 1.0);
    }
    // The clone must leave the shared mat-anim table: its channels are ticked per
    // INSTANCE, and a carried world slot would add the shared delta on top — a double animation
    // the old asset-mutating lane could never produce. `register_fx_uv` below hands it rows of
    // its own.
    mat.extension.anim_slots = Vec4::ZERO;
    if loops.any() {
        // **Seed the clone at the loop's own t = 0**, not at the entity lane's flat zero
        // (`entity_variants` builds every effect batch with `play_uv = false`, so the shared
        // steady this was cloned from carries no offset at all). The delta table is measured from
        // whatever `sun_scale.zw` holds at registration (1381), so seeding here is what makes the
        // registration's own opening row land on the scroll's first frame.
        let t0 = loops.open_offset();
        mat.extension.sun_scale.z = t0[0];
        mat.extension.sun_scale.w = t0[1];
    }
    let handle = mats.store.add(mat);
    if let Some(anim) = &part.rgb_anim {
        mats.tint.0.insert(handle.id(), (anim.clone(), now));
    }
    if loops.any() {
        benilla_world::doodad_anim::register_fx_uv(
            mats.uv,
            mats.table,
            mats.store,
            handle.id(),
            loops,
            host,
            now,
        );
    }
    handle
}

// (The ground-decal material clone died with 0733: a ground-quad part's draw identity —
// texture, blend, fog policy, RGB/alpha loops — rides its `GroundFxDecal` record and the
// effect stream's per-vertex tint instead of a per-instance `WowModelMaterial`.)

/// Where an effect-model instance sits in the client's **model graph** — the two facts every
/// caller of [`attach_effect_visuals`] states, together, because they are one fact seen from two
/// sides and a lane that answered only the first is what left a weapon's enchant glow with no
/// alpha source and no owner.
#[derive(Clone, Copy, Default)]
pub(crate) struct EffectHost {
    /// The model instance this one is **chained to** ([`benilla_world::model_fade::ParentModel`]): the
    /// unit a kit effect is hung on, the item root a weapon glow rides. `None` for a model that
    /// belongs to no other — a missile, the fixture preview — which is also what keeps the 0202
    /// drain for the impacting trail.
    pub parent: Option<Entity>,
}

/// Attach one effect model's full visual set to `root` — THE one body for every effect-model
/// consumer (spell-kit instances, missiles, the `fxview` capture fixture): the authored rig
/// ([`arm_effect_rig`]), the part meshes (skinned twins on a rigged model, `NoFrustumCulling` —
/// a billboarded subtree renders wherever the camera is, so a bind-pose Aabb is wrong by
/// construction), the billboard cards (riding their bone's joint on a rigged model, decisions
/// 0153/0154), the emitters/ribbons on their host-bone joints (the 0130 rig identity), the
/// **material animation** (module docs): per-part `MatAnim` alpha samplers on the attach clock
/// (`now`) + per-instance tint clones for animated-RGB parts — and, on a **ground-anchored**
/// instance (`ground_anchor`: the kit slot resolved to the base point `0x13` or the unit root),
/// each flat ground-plane quad part as a projected surface decal ([`crate::ground_fx`]) riding
/// its joint, so Battle Shout's crescents drape sloped terrain instead of being buried by it.
/// `host` says where the instance sits in the model graph ([`EffectHost`]) — which decides its
/// emitters' attach frame, whose render alpha they inherit, and what becomes of them when the
/// instance goes. Returns `false` while the model's parts are still building — call again next
/// frame.
///
/// `stage` is the instance's **animation lifecycle** ([`lifecycle`]): `Some` for a spell-visual
/// `CEffect`, which advances `Stand` → `Hold` → `Decay` and therefore reads its per-sequence riders
/// (each part's alpha loops, each emitter's rate/enable windows) off its own live `AnimationPlayer`
/// rather than off a slot pinned at spawn. `None` for the lanes that are not `CEffect`s and never
/// advance — a missile (the separate `CMissile` TU), an item glow, the `fxview` preview — which
/// keep the pinned single-clip arm they have always had.
pub(crate) fn attach_effect_visuals(
    commands: &mut Commands,
    root: Entity,
    dm: &DisplayModel,
    now: f32,
    ground_anchor: bool,
    host: EffectHost,
    stage: Option<FxStage>,
    mats: &mut FxMaterials,
    ibps: &Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>,
    palettes: &mut benilla_world::rig_palette::RigPalettes,
    preferred_anim: Option<u16>,
) -> bool {
    let Some(parts) = dm.parts.as_ref() else {
        return false; // still loading — attach on a later pass
    };
    // Chain this instance onto the model it hangs from, so its effects compose through the
    // parent's computed alpha the way `0x714000` does — a weapon's glow through the item, the
    // item through its wearer.
    if let Some(parent) = host.parent {
        commands
            .entity(root)
            .insert(benilla_world::model_fade::ParentModel(parent));
    }
    // …and, from the same fact, what losing the owner means: an instance hung on another model is
    // part of that model's tree, so its pool is freed with it (the reference's dtor) instead of
    // being left to finish in world space — the ghost clouds a gear change used to strand at the
    // body. A free-standing instance (a missile) keeps the 0202 drain.
    let on_owner_loss = if host.parent.is_some() {
        benilla_world::particles::OwnerLoss::Free
    } else {
        benilla_world::particles::OwnerLoss::Drain
    };
    let is_ground_decal = |part: &EntityPart| ground_anchor && part.ground_quad.is_some();
    // Per-part materials for THIS instance (a tint clone where the RGB animates), resolved
    // before the child-spawn closure so the material assets aren't borrowed inside it.
    // Ground-decal parts keep the shared handle unused — their identity rides the decal record.
    let part_materials: Vec<Handle<WowModelMaterial>> = parts
        .iter()
        .map(|p| {
            if is_ground_decal(p) {
                p.material.clone()
            } else {
                fx_part_material(p, now, root, mats)
            }
        })
        .collect();
    let (joints, armed) = arm_effect_rig(commands, root, dm, preferred_anim, stage);
    // The owned palette rig: allocated when the effect draws skinned parts; the
    // hook frees the slot when the instance despawns (impact reap, missile arrival).
    let rig_slot = match (&dm.inverse_bindposes, joints.is_empty()) {
        (Some(ibp), false) => {
            benilla_world::rig_palette::RigSkin::allocate(palettes, joints.clone(), ibp.clone())
                .map_or(0, |rig| {
                    let slot = rig.slot;
                    commands.entity(root).insert(rig);
                    slot
                })
        }
        _ => 0,
    };
    let rigged = rig_slot != 0;
    // The clip this instance OPENS on (the missile's InFlight, else the model's `Stand`) — the same
    // pick `arm_effect_rig` armed. Its **file sequence slot** is the key into each batch's
    // per-sequence material-alpha loops: an effect that plays a non-first sequence must read that
    // sequence's authored batch visibility, not sequence 0's.
    let played = dm
        .animations
        .as_ref()
        .and_then(|a| a.preferred_clip(preferred_anim));
    let played_seq = played.map(|c| c.seq_index);
    // …and for a `CEffect`, that is only the OPENING slot: the instance advances to `Hold` and then
    // to `Decay`, each with its own authored alpha loops and emitter windows (for IceShield the
    // pulse is as much the transparency-weight tracks as the bone scale). So its per-sequence
    // riders read the live sequence off this instance's own player each frame — the same
    // `playing_seq` lane a creature's batches already use — instead of the slot they opened on.
    // A lane with no rig has no player to ask and stays pinned, as before.
    let seq_host = (armed && stage.is_some()).then_some(root);
    commands.entity(root).with_children(|children| {
        for (part, material) in parts.iter().zip(&part_materials) {
            if part.billboard.is_some() {
                continue; // spawned as a following card below (decision 0153)
            }
            if is_ground_decal(part) {
                continue; // spawned as a projected surface decal below
            }
            let mesh = match (rigged, &part.skinned_mesh) {
                (true, Some(sm)) => sm.clone(),
                _ => part.mesh.clone(),
            };
            let mut child = children.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material.clone()),
                Transform::default(),
                bevy::camera::visibility::NoFrustumCulling,
                ModelPart {
                    kind: ModelKind::Creature,
                    blend: part.blend,
                },
                // The picker's triangles: the probe names fx batches through
                // `ModelPart`, and the render meshes are `RENDER_WORLD`-only.
                benilla_world::interact::PickMesh(part.geometry.clone()),
            ));
            if let (true, Some(_)) = (rigged, &part.skinned_mesh) {
                child.insert((
                    benilla_world::rig_palette::RigPart(root),
                    bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(rig_slot, 1.0)),
                ));
            }
            // The part's colour-alpha × weight loops, on this instance's clock: the sampler owns
            // the child's render-alpha tag (`drives_tag` — no other writer touches fx parts).
            // The rig field rides the whole-tag seed.
            if let Some(anim) = &part.alpha_anim {
                let mat_anim =
                    benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), now, played_seq)
                        .following_host(seq_host);
                let tag_slot = if rigged && part.skinned_mesh.is_some() {
                    rig_slot
                } else {
                    0
                };
                child.insert((
                    bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(
                        tag_slot,
                        mat_anim.current,
                    )),
                    mat_anim,
                ));
            }
        }
    });
    for (part, material) in parts.iter().zip(&part_materials) {
        let Some(info) = &part.billboard else {
            continue;
        };
        let card = match joints.get(info.bone as usize) {
            Some(&j) => benilla_world::billboard::BillboardCard::following_joint(info, j),
            None => benilla_world::billboard::BillboardCard::following(info, root),
        };
        let mut spawned = commands.spawn((
            Mesh3d(part.mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::default(),
            ModelPart {
                kind: ModelKind::Creature,
                blend: part.blend,
            },
            // The picker's triangles, pivot-centred by the caster like the bake.
            benilla_world::interact::PickMesh(part.geometry.clone()),
            card,
        ));
        // The card's build-time bound: `calculate_bounds` can no longer derive
        // one from the `RENDER_WORLD`-only static form's data.
        if let Some(aabb) = part.aabb {
            spawned.insert(aabb);
        }
        // A card shares its batch's material-alpha loops (the billboard split copies them).
        if let Some(anim) = &part.alpha_anim {
            let mat_anim =
                benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), now, played_seq)
                    .following_host(seq_host);
            spawned.insert((
                bevy::mesh::MeshTag(benilla_world::mesh_tag::spawn_tag(0, mat_anim.current)),
                mat_anim,
            ));
        }
    }
    // Ground-plane quad parts of a ground-anchored instance → projected surface decals
    // ([`crate::ground_fx`]): each rides its own joint (posed through the bone's inverse
    // bindpose, exactly the skinned-vertex path). The part's draw identity — texture, blend,
    // the shared material's baked `0x70baf0` fog policy, its RGB loop — rides the decal record;
    // the alpha loop stays a `MatAnim` rider whose `current` the push samples.
    let binds = dm.inverse_bindposes.as_ref().and_then(|h| ibps.get(h));
    for part in parts.iter() {
        let Some(quad) = part.ground_quad.filter(|_| is_ground_decal(part)) else {
            continue;
        };
        let (joint, ibp) = match joints.get(quad.bone as usize) {
            Some(&j) => (
                j,
                binds
                    .and_then(|b| b.get(quad.bone as usize))
                    .copied()
                    .unwrap_or(Mat4::IDENTITY),
            ),
            None => (root, Mat4::IDENTITY), // boneless model: the quad rides the instance root
        };
        // The material's packed fog byte, handed over raw — the lane owns what it maps to.
        // `7` (Scene) is the no-material fallback the packer's own default agrees with.
        benilla_world::model_render::lazy::realize(mats.store, part.material.id());
        let (texture, fog_bits) = match mats.store.get(part.material.id()) {
            Some(mat) => (
                mat.base.base_color_texture.clone(),
                (mat.extension.clutter_fade.z as u32 >> 4) & 7,
            ),
            None => (None, u32::from(benilla_formats::FogPolicy::Scene as u8)),
        };
        let Some(texture) = texture else {
            continue; // texture-less ground quad: nothing the stream could draw
        };
        let decal = benilla_world::ground_fx::spawn_ground_fx_decal(
            commands,
            texture,
            part.blend,
            part.additive,
            fog_bits,
            part.rgb_anim.as_ref().map(|a| (a.clone(), now)),
            &quad,
            joint,
            ibp,
        );
        if let Some(anim) = &part.alpha_anim {
            let mat_anim =
                benilla_world::doodad_anim::MatAnim::driving_tag(anim.clone(), now, played_seq)
                    .following_host(seq_host);
            commands.entity(decal).insert(mat_anim);
        }
    }
    for em in &dm.emitters {
        let owner = joints
            .get(em.def.bone as usize)
            .map_or((root, [0.0; 3]), |&j| (j, em.bone_pivot));
        particles::spawn_emitter(
            commands,
            em,
            Transform::IDENTITY,
            particles::EmitterFrames {
                owner: Some(owner),
                // The cloud SORTS at the instance root — the emitter's bone only composes births.
                // Since 1585 that is all this is: a world-mode store is frozen at birth, so
                // neither this root nor the bone can move a particle that already exists.
                anchor: Some(root),
                // Free with the model when this instance belongs to one, drain when it stands
                // alone (the impacting missile's trail — 0202's case). See `on_owner_loss` above.
                on_owner_loss,
                // This instance IS the model these particles belong to; its chain (set above)
                // carries the host's fade down to them.
                alpha: Some(root),
                // A spell effect has no light node wired: the whole `Spells\` corpus SETS the
                // emitter unlit bit, so nothing here consumes one today. A lit one would fall
                // back to the scene's light, which is what it takes now.
                light_node: None,
            },
            // The emitters' rate/enabled windows ride the played sequence: a `CEffect` ADVANCES
            // (`Stand` → `Hold` → `Decay`), so it reads the live one off its own player like a
            // unit's do — the phase in the reference's animate kernel `0x714260` samples the
            // CURRENT sequence record either way. A lane that never advances (a missile, whose
            // InFlight is not the model's Stand) keeps its slot pinned on the spawn clock. gseq
            // loops ride the instance's own spawn age in both (an effect instance is
            // fresh per play).
            match seq_host {
                Some(h) => particles::EmitClock::Host(h),
                None => particles::EmitClock::Effect(played_seq),
            },
        );
    }
    for rb in &dm.ribbons {
        let (owner, use_pivot) = joints
            .get(rb.def.bone as usize)
            .map_or((root, false), |&j| (j, true));
        benilla_world::ribbons::spawn_ribbon(
            commands,
            rb,
            owner,
            use_pivot,
            // An effect-model instance is armed unscaled — its emitters take the same
            // `Transform::IDENTITY` placement a few lines up, so both of a model's effect
            // families land on the same draw-order rung, which is the property that matters.
            1.0,
            // The instance's own root carries the clip: an effect model that steps
            // Stand -> Hold -> Decay re-answers the gate at each step, instead of freezing the
            // birth clip's answer for the whole life.
            benilla_world::ribbons::RibbonSeq::Host(root),
            // This instance's own model alpha, chained to its host: a standalone
            // instance has none above it and draws exactly as before.
            Some(root),
            // No fade sphere: an effect instance is not a placed model — it lives for its
            // clip and is gated by its own model alpha one line up.
            None,
        );
    }
    true
}

/// Arm an effect-model instance's **authored rig** under `root` (a missile entity / fx instance
/// root): the joint hierarchy, an `AnimationPlayer` on the model's birth clip, and the
/// free-running global-sequence channels. Effect models pose their emitter/ribbon/mesh bones with
/// this rig — the fireball missile's constant bone keys turn its authored long-axis-up frame into
/// flight-forward and set its two trail ribbons 90° apart, and its global-sequence bone tumbles
/// the molten core. The clip plays regardless of the doodad content gate
/// (`ModelAnimations::first_seq`, which skips constant-pose sequences as render-identical — true
/// for skinned MESHES at bind pose, but an effect's emitters read the posed joints, so the
/// constant keys are load-bearing here).
///
/// The clip is the reference's model-load bootstrap — **animation id 0 `Stand`**, not the
/// file-order-first slot ([`benilla_assets::ModelAnimations::preferred_clip`]) — and for a
/// `CEffect` (`stage` is `Some`) it is only the OPENING leg: [`FxAnimLife`] carries the completion
/// callback that advances it to `Hold` and, at the reap, to `Decay` ([`lifecycle`]).
///
/// Returns the joint entities (empty for a boneless model — everything then rides `root` directly,
/// as before) and whether an `AnimationPlayer` was actually armed, which is what decides whether
/// the per-sequence riders have a live sequence to follow.
pub(super) fn arm_effect_rig(
    commands: &mut Commands,
    root: Entity,
    dm: &DisplayModel,
    preferred_anim: Option<u16>,
    stage: Option<FxStage>,
) -> (Vec<Entity>, bool) {
    if dm.skeleton.joints.is_empty() {
        return (Vec::new(), false);
    }
    let joints = benilla_world::rig_palette::spawn_joints(commands, root, root, &dm.skeleton);
    // Billboard bones face the camera at the PALETTE level, children inheriting (the frost-armor
    // sheets skin to a lock-Z bone's child) — the joint pass needs the map.
    if let Some(bb) = benilla_world::billboard::BillboardJointRig::new(&dm.skeleton, &joints, root)
    {
        commands.entity(root).insert(bb);
    }
    let mut armed = false;
    if let Some(anims) = dm.animations.as_ref() {
        // A caller can name the sequence this instance should run (the thrown-weapon missile asks
        // for InFlight, so its authored spin plays and its ribbon's per-sequence visibility keys
        // ON); absent that — or when the model has no such sequence (an arrow, a fireball whose
        // tumble is a global sequence) — the bootstrap's `Stand`.
        if let Some(clip) = anims.preferred_clip(preferred_anim) {
            let mut player = AnimationPlayer::default();
            // The stage owns the repeat policy, not the sequence flag alone: stages 3/4 re-arm
            // unconditionally (`0x60ed00`), so a clamping precast still repeats.
            let life = FxAnimLife::arm(&mut player, clip, stage.unwrap_or(FxStage::OneShot));
            commands
                .entity(root)
                .insert((player, AnimationGraphHandle(anims.graph.clone())));
            // The lifecycle watcher, and the `ModelAnimations` it (and the per-sequence riders)
            // resolve the live sequence through — only on the `CEffect` lanes; a missile or an
            // item glow neither advances nor is ever decay-reaped.
            if stage.is_some() {
                commands.entity(root).insert((life, anims.clone()));
            }
            for (i, &j) in joints.iter().enumerate() {
                commands
                    .entity(j)
                    .insert((bone_target_id(i as u16), AnimatedBy(root)));
            }
            armed = true;
        }
        if let Some(drive) =
            benilla_world::rig_anim::GlobalSeqDrive::new(&anims.global_bones, &joints)
        {
            // Fresh anchor per play — the byte-verified effect lifecycle: CreateModel
            // always allocates+attaches, so every cast's gseq loops open at phase 0, exactly
            // like the ref (the director's 3-cast trace: the flash at +16 frames every time).
            commands.entity(root).insert(drive);
        }
    }
    (joints, armed)
}

/// One live (or pending) effect-model instance on a unit.
struct FxInstance {
    /// The spell that owns it — the reap key (the client stores it at `CEffect+0x18`). The
    /// lootable-corpse sparkle reserves `u32::MAX` (`spell_visual::LOOT_FX_KEY` — not a spell).
    spell_id: u32,
    /// Precast/channel lifetime (reaped by spell id) vs cast-release (self-terminates).
    persistent: bool,
    /// Which owner's reap can kill a persistent instance (cast router vs aura watcher) — the
    /// client's reap-walk discriminator next to the spell id ([`FxClass`]).
    class: FxClass,
    /// Which animation lifecycle its model runs ([`FxStage`]) — a separate axis from `class`, and
    /// carried per instance because the same `class` covers a stage-4 precast and a stage-2
    /// channel.
    stage: FxStage,
    /// The M2 attachment id to hang from ([`benilla_formats::KIT_SLOT_TAGS`]), or
    /// [`benilla_formats::WORLD_EFFECT_TAG`] for the field-12 world-plant slot.
    tag: u16,
    /// The `SpellVisualEffectName` record id — with [`Self::tag`], the reference's same-slot
    /// replace key (`0x6208e0`; [`replace_same_slot`]).
    effect: u32,
    /// The model-cache key.
    path: String,
    /// The spawned instance root (a child of the attach joint), `None` while the model loads.
    root: Option<Entity>,
    /// The self-termination deadline (`time.elapsed_secs()` clock), set at spawn for a
    /// non-persistent instance — and, for a reaped persistent one, the end of its `Decay` span.
    expires: Option<f32>,
    /// Reaped, and playing its `Decay` out (`0x614150`/`0x6141f0`: the node is not torn down
    /// synchronously — it keeps rendering for that sequence's authored duration). Such an
    /// instance is already dying, so it no longer answers a reap or a replacing `Begin`: the
    /// reference's walk can't reach it either, having moved it to the pending-destroy list.
    decaying: bool,
}

/// A unit's effect instances (the client's per-unit `+0xb4` effect list).
#[derive(Component, Default)]
pub(super) struct FxAttached {
    instances: Vec<FxInstance>,
}

/// A **world-planted** kit instance root (the field-12 slot): a free world
/// entity, NOT a scene child of its owner — so [`tend_world_plants`] owns the two jobs the tree
/// would otherwise do. The reference plants it once at the owner's position × yaw × scale
/// (`0x620a90`); a **root-aura** spell's plant (`EffectApplyAuraName` 26 anywhere in the spell —
/// the client's flag 0x4000) re-plants when the owner is displaced (knockback, blink;
/// `0x620580`), re-baking facing and scale.
#[derive(Component)]
pub(super) struct WorldPlantFx {
    /// The unit the instance belongs to — despawn tracking (the client's node dies with its
    /// owner) and the re-plant source.
    owner: Entity,
    /// Re-plant on owner displacement (the 0x4000 leg) — root-aura persistents only.
    follow: bool,
}

/// The client's re-plant displacement gate: distance² > 1e-3 (`0x620580`'s squared-distance
/// compare; world units).
const REPLANT_EPS_SQ: f32 = 1e-3;

/// The plant transform (`0x620a90`): `translate(owner position) × yaw(owner facing about the up
/// axis) × scale(owner scale)` — yaw ONLY (a swimming or mount-tilted body plants level), baked
/// at spawn.
fn world_plant_transform(owner: &GlobalTransform) -> Transform {
    let (scale, rotation, translation) = owner.to_scale_rotation_translation();
    let (yaw, _, _) = rotation.to_euler(EulerRot::YXZ);
    Transform {
        translation,
        rotation: Quat::from_rotation_y(yaw),
        scale,
    }
}

/// Whether any of the spell's three `EffectApplyAuraName` slots is `SPELL_AURA_MOD_ROOT` (26) —
/// the client's `spellRec+0x16c[0..2] == 0x1a` scan that arms the re-plant flag 0x4000
/// (`0x60f081`; only the column *name* is inferred).
/// Exactly the field-12 state family: Frost Nova, Net, Web, Entangling Roots, Frostbite.
fn spell_has_root_aura(spells: Option<&crate::ui_action::Spells>, spell_id: u32) -> bool {
    const SPELL_AURA_MOD_ROOT: u32 = 26;
    spells
        .and_then(|s| s.catalog.get(spell_id))
        .is_some_and(|d| d.effect_apply_aura.contains(&SPELL_AURA_MOD_ROOT))
}

/// Tend the live world plants: despawn a plant whose owner is gone (a scene child would get this
/// from the tree; a free root must be swept), and re-plant a `follow` instance whose owner was
/// displaced past [`REPLANT_EPS_SQ`] — the client's `0x620580` per-frame leg, which re-bakes
/// position, facing AND scale on displacement.
pub(super) fn tend_world_plants(
    mut commands: Commands,
    mut plants: Query<(Entity, &WorldPlantFx, &mut Transform)>,
    owners: Query<&GlobalTransform>,
) {
    for (root, plant, mut t) in &mut plants {
        let Ok(owner) = owners.get(plant.owner) else {
            commands.entity(root).despawn();
            continue;
        };
        if plant.follow && owner.translation().distance_squared(t.translation) > REPLANT_EPS_SQ {
            *t = world_plant_transform(owner);
        }
    }
}

/// Consume the router's [`SpellKitFx`] edges: `Begin` records instances (and creates their model
/// cache entries), `Reap` despawns the matching spell's persistent instances. Ordered per unit —
/// a GO's reap-then-begin lands in emission order, so the precast dies before the release flash
/// spawns.
pub(super) fn resolve_spell_fx(
    mut commands: Commands,
    mut events: MessageReader<SpellKitFx>,
    mut units: Query<&mut FxAttached>,
    fx: Option<ResMut<SpellFx>>,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
    mut emitters: Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    let Some(mut fx) = fx else { return };
    let now = time.elapsed_secs();
    // Group the edges per unit, preserving order, so each unit's instance list is written once.
    let mut ops: EntityHashMap<Vec<&SpellKitFx>> = EntityHashMap::default();
    for ev in events.read() {
        let entity = match ev {
            SpellKitFx::Begin { entity, .. } | SpellKitFx::Reap { entity, .. } => *entity,
        };
        ops.entry(entity).or_default().push(ev);
    }
    for (entity, edges) in ops {
        // A unit may have despawned between the edge and this frame — drop its edges.
        if commands.get_entity(entity).is_err() {
            continue;
        }
        let existing = units.get_mut(entity).ok();
        let mut instances = match existing {
            Some(mut att) => std::mem::take(&mut att.instances),
            None => Vec::new(),
        };
        for edge in edges {
            match edge {
                SpellKitFx::Begin {
                    spell_id,
                    persistent,
                    class,
                    stage,
                    effects,
                    ..
                } => {
                    // A persistent Begin REPLACES the unit's live persistent instances of the
                    // same (spell, class) — so an aura whose remove/add edges land across frames
                    // (a re-apply) re-arms cleanly instead of stacking. The replaced instance
                    // decays out exactly as a reaped one does: in the reference these are two
                    // nodes, the old one dying on its own clock while the new one is born.
                    if *persistent {
                        reap_matching(
                            &mut instances,
                            *spell_id,
                            *class,
                            &fx,
                            now,
                            &mut commands,
                            &mut emitters,
                        );
                    }
                    for FxSlot { tag, effect, path } in effects {
                        // **The same-slot replace**, run per slot exactly where the reference
                        // runs it: `CEffect::AddEffect 0x61fdd0` opens with
                        // `0x6208e0(owner, rec, tag)`.
                        replace_same_slot(
                            &mut instances,
                            *effect,
                            *tag,
                            &mut commands,
                            &mut emitters,
                        );
                        ensure_model(&mut fx, &asset_server, path);
                        instances.push(FxInstance {
                            spell_id: *spell_id,
                            persistent: *persistent,
                            class: *class,
                            stage: *stage,
                            tag: *tag,
                            effect: *effect,
                            path: path.clone(),
                            root: None,
                            expires: None,
                            decaying: false,
                        });
                    }
                }
                SpellKitFx::Reap {
                    spell_id, class, ..
                } => {
                    reap_matching(
                        &mut instances,
                        *spell_id,
                        *class,
                        &fx,
                        now,
                        &mut commands,
                        &mut emitters,
                    );
                }
            }
        }
        match units.get_mut(entity) {
            Ok(mut att) => att.instances = instances,
            Err(_) => {
                commands.entity(entity).insert(FxAttached { instances });
            }
        }
    }
}

/// **Hand an ending instance's emitters to the drain** — the half of the reference's teardown
/// `0x6203e0` that is not a despawn.
///
/// An ending `CEffect` is *hidden*, not freed: emission stops, the already-emitted particles keep
/// drawing and age out one at a time, and the node is released only on the frame the last one dies
/// — the draw path's own admission test is the live-particle count itself (`0x7b4b46 mov
/// eax,[esi+0x64]`), which nothing in the teardown touches. Despawning the root outright, as this
/// lane did before, cut every live particle at once: for `Strike_Impact_Chest` a replace landing
/// mid-burst threw away up to 0.30 s of a 0.334 s effect, as a pop mid-ramp.
///
/// The meshes are right to go with the root: the reference drops the model off the MESH draw list
/// the same frame (`0x719207` gates it on the visible flag the teardown clears) while the PARTICLE
/// list is gated on the busy mask (`0x71922c`), which the survivors keep set.
///
/// Emitters are free entities carrying their instance root as [`ParticleEmitter::anchor`], so that
/// is the identity here — a linear scan, run only when an instance actually ends (a few per second
/// in a fight, against a pool the sim already walks every frame).
fn drain_instance_emitters(
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
    root: Entity,
) {
    for mut e in emitters.iter_mut() {
        if e.anchor() == Some(root) {
            e.drain_on_owner_loss();
        }
    }
}

/// **The same-slot replace walk** (`0x6208e0`, `[0x6208e0, 0x62092c]`): every
/// `CEffect::AddEffect` (`0x61fdd0`) opens by walking the owner's `+0xb4` list and **destroying**
/// (`0x6203e0`) each node carrying the same `SpellVisualEffectName` record at the same attach tag,
/// so a re-play *replaces* a still-live same-model-same-slot emitter instead of stacking on it.
///
/// This is the whole reason a busy fight does not brighten without bound in the reference: five
/// mobs' blood spurts on one flank of one body are one instance there, not five, and the count is
/// **flat in the number of attackers** rather than linear. It is a *destroy*, not the reap's
/// decay — the node is gone the same frame, and the newcomer is born in its place.
///
/// Two exclusions, both the reference's own:
/// - **`tag == -1`** ([`benilla_formats::WORLD_EFFECT_TAG`], the field-12 world plant) is skipped
///   at `0x620913`, so successive world plants genuinely coexist.
/// - an instance already **decaying** is not reachable: the reference has moved it off `+0xb4`
///   onto the pending-destroy list, exactly as [`reap_matching`] documents.
fn replace_same_slot(
    instances: &mut Vec<FxInstance>,
    effect: u32,
    tag: u16,
    commands: &mut Commands,
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    if tag == benilla_formats::WORLD_EFFECT_TAG {
        return;
    }
    instances.retain(|i| {
        if i.decaying || i.effect != effect || i.tag != tag {
            return true;
        }
        if let Some(root) = i.root {
            // The replaced node's particles finish — the replace bounds the number of EMITTING
            // nodes, not the number of live particles (it ends the node through the shared
            // teardown `0x6203e0`).
            drain_instance_emitters(emitters, root);
            commands.entity(root).despawn();
        }
        false
    });
}

/// The spell-id-keyed reap walk (`0x614150`): every live persistent instance of `(spell_id, class)`
/// **plays its `Decay` out** and is torn down when that span ends — the reference's three gates in
/// order (`0x614187`–`0x6141a1`: a model handle, a ready model, and a `Decay` the model actually
/// authors), with an immediate destroy whenever any of them fails.
///
/// An instance already decaying is skipped: it is dying, and in the reference it has been moved off
/// the unit's `+0xb4` list to the pending-destroy list, so a second reap cannot reach it either.
/// One that never spawned a root has nothing to play and goes at once.
fn reap_matching(
    instances: &mut Vec<FxInstance>,
    spell_id: u32,
    class: FxClass,
    fx: &SpellFx,
    now: f32,
    commands: &mut Commands,
    emitters: &mut Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    instances.retain_mut(|i| {
        if i.decaying || !i.persistent || i.spell_id != spell_id || i.class != class {
            return true;
        }
        let span = i
            .root
            .and_then(|_| fx.models.get(&i.path))
            .and_then(|dm| decay_span(dm.animations.as_ref()));
        let (Some(root), Some(span)) = (i.root, span) else {
            if let Some(root) = i.root {
                // A reap with no `Decay` to play destroys at once — and that destroy is
                // `0x6203e0` like any other, so its particles still drain.
                drain_instance_emitters(emitters, root);
                commands.entity(root).despawn();
            }
            return false;
        };
        commands.entity(root).try_insert(FxDecay);
        i.decaying = true;
        i.expires = Some(now + span);
        lifecycle::trace_leg("decay", root, lifecycle::ANIM_DECAY);
        true
    });
}

/// Spawn pending instances whose model finished building, and run the self-termination clock.
/// The instance root is a child of the attach joint at the attachment offset (the cascade above),
/// so the whole effect — meshes and particle emitters alike — rides the animating bone. The one
/// exception is the kit's **world-plant slot** ([`benilla_formats::WORLD_EFFECT_TAG`], kit field
/// 12): its root is a free world entity at the owner's position/facing/scale, [`WorldPlantFx`].
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(super) fn attach_spell_fx(
    mut commands: Commands,
    mut units: Query<(
        Entity,
        &mut FxAttached,
        Option<&BoneAttach>,
        // The unit's pose buffer: the attach joint spawns on first demand from the composed
        // pose (`RigPose::anchor_for`).
        Option<&mut benilla_world::rig_anim::RigPose>,
        &GlobalTransform,
        // The body is built (model or fallback cube) — see the wait below.
        Has<crate::entities::VisualAttached>,
    )>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    spells: Option<Res<crate::ui_action::Spells>>,
    time: Res<Time>,
    mut wow_materials: ResMut<Assets<WowModelMaterial>>,
    mut tint_reg: ResMut<FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
    mut emitters: Query<&mut benilla_world::particles::ParticleEmitter>,
) {
    let Some(mut fx) = fx else {
        return;
    };
    let now = time.elapsed_secs();
    for (unit, mut att, bones, mut pose, unit_gt, body_built) in &mut units {
        att.instances.retain_mut(|inst| {
            // Self-termination (the cast-release flash ran its span).
            if let Some(expires) = inst.expires {
                if now >= expires {
                    if let Some(root) = inst.root {
                        // The flash ran its span — but its particles have their own clocks and
                        // outlive it (`drain_instance_emitters`). This is the completion
                        // callback's leg of the same `0x6203e0` teardown the replace takes.
                        drain_instance_emitters(&mut emitters, root);
                        commands.entity(root).despawn();
                    }
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line(
                            "fx",
                            &format!("kit expire unit={unit} path={}", inst.path),
                        );
                    }
                    return false;
                }
            }
            // A root that died as **collateral**: [`FxAttached`] lives on the unit and outlives
            // its model — exactly as the reference's per-CGUnit `+0xb4` effect list does — so a
            // teardown of the unit's visual (a live display swap, `live_display`; before 0835 a
            // gear change; before B199's fix a mount transition) leaves the instance holding a
            // dangling entity, and this gate's `root.is_some()` then means it is never rebuilt.
            // The reference's rebuild `0x60abe0` DRAINS the whole `+0xb4` list unconditionally
            // and then RE-CREATES what must persist — the aura-state re-arm `0x5ff130`, the
            // channel re-arm `0x612a30`. Our edge-driven aura watcher cannot notice
            // (the aura never left the slots), so re-arming the PERSISTENT instance here IS that
            // re-create leg, in the one place that sees the drain. A one-shot goes, exactly as
            // the reference's drained Mount Poof does — the shapeshift cloud is NOT survival:
            // it is the pending-morph latch's impact-kit REPLAY after the rebuild
            // (`creature_anim::spell_visual::replay_morph_kit`), and a decaying instance was
            // ending anyway.
            if let Some(root) = inst.root {
                if commands.get_entity(root).is_ok() {
                    return true;
                }
                if !inst.persistent || inst.decaying {
                    if benilla_assets::trace::enabled() {
                        benilla_assets::trace::line(
                            "fx",
                            &format!(
                                "kit collateral drain unit={unit} path={} decaying={}",
                                inst.path, inst.decaying
                            ),
                        );
                    }
                    return false;
                }
                inst.root = None;
                if benilla_assets::trace::enabled() {
                    benilla_assets::trace::line(
                        "fx",
                        &format!("kit collateral re-create unit={unit} path={}", inst.path),
                    );
                }
            }
            // Still pending: a self-terminating instance gets a generous deadline so a unit whose
            // model never materialises (the cube fallback) can't accumulate unspawnable flashes —
            // persistent ones are reaped by their spell edge regardless. A successful spawn below
            // overwrites this with the real clip span.
            if !inst.persistent && inst.expires.is_none() {
                inst.expires = Some(now + PENDING_TIMEOUT);
            }
            ensure_model(&mut fx, &asset_server, &inst.path);
            let Some(dm) = fx.models.get(&inst.path) else {
                return true; // unreachable — just inserted
            };
            if dm.parts.is_none() {
                return true; // model still loading — spawn on a later pass
            }
            // The world-plant slot (kit field 12): the client passes NO
            // attach tag (`0x61fcf0` pushes −1), skips the bone pipeline entirely and plants the
            // model once, world-space, at the owner's position × yaw × scale (`0x620c86` via
            // `0x620a90`) — it does not ride a bone and does not turn with the unit afterwards.
            // A root-aura spell's plant additionally re-plants on owner displacement
            // ([`tend_world_plants`] — the client's 0x4000 flag leg).
            let planted = inst.tag == benilla_formats::WORLD_EFFECT_TAG;
            // **A bone-riding effect waits for the body.** The cascade below reads the unit's
            // attach points, and before the body is built (a display swap still loading the new
            // model, a player still settling its equipment) there are none — so it fell through
            // to the unit root, ground-anchored, and a persistent instance is never re-seated
            // after that: an Ice Barrier or a Power Word: Shield sat decaled at the feet for the
            // aura's whole life. The reference re-arms the persistent kits after the rebuild has
            // set the new model (`0x60abe0` → `0x5ff130`);
            // `VisualAttached` is that "the model is set" — the cube fallback carries it too, so
            // a unit whose model never loads still gets its effect at the root.
            if !planted && !body_built {
                return true;
            }
            // Ground-anchored: a world plant sits at the feet by construction; otherwise the
            // cascade landed on the model's BASE point (`0x13`) or fell through to the unit
            // root — the feet-level anchors. A hand/head/chest-anchored instance keeps its flat
            // quads as ordinary geometry (the ProtectionFrom* chest shields author flat quads
            // too — they must NOT decal to the ground).
            let (root, ground_anchor) = if planted {
                let follow =
                    inst.persistent && spell_has_root_aura(spells.as_deref(), inst.spell_id);
                let root = commands
                    .spawn((
                        world_plant_transform(unit_gt),
                        Visibility::default(),
                        WorldPlantFx {
                            owner: unit,
                            follow,
                        },
                    ))
                    // Chain-only visibility (benilla_world::vis_chain) — the wrapper
                    // renders nothing; the effect's parts are the children.
                    .vis_chain_only()
                    .id();
                (root, true)
            } else {
                // The attach cascade: the slot's tag, the client's two fallbacks, else the unit
                // root.
                let point = bones.and_then(|b| {
                    std::iter::once(inst.tag)
                        .chain(ATTACH_FALLBACKS)
                        .find_map(|tag| b.points.get(&tag).copied().map(|p| (tag, p)))
                        .and_then(|(tag, (bone, offset))| {
                            pose.as_mut()
                                .and_then(|p| p.anchor_for(&mut commands, unit, bone))
                                .map(|joint| (tag, joint, offset))
                        })
                });
                let ground_anchor = point.is_none_or(|(tag, ..)| tag == 0x13);
                let (parent, offset) = point.map_or((unit, Vec3::ZERO), |(_, j, o)| (j, o));
                let root = commands
                    .spawn((Transform::from_translation(offset), Visibility::default()))
                    // Chain-only, as above.
                    .vis_chain_only()
                    .id();
                commands.entity(parent).add_child(root);
                (root, ground_anchor)
            };
            // The one shared effect-visuals body — rig, skinned parts, joint-riding
            // cards/emitters/ribbons, ground decals, material animation. Parts were checked
            // ready above, so this always attaches.
            attach_effect_visuals(
                &mut commands,
                root,
                dm,
                now,
                ground_anchor,
                // A kit instance on a unit is an attached model, chained to that unit: it fades
                // with the body it is cast on, and it is freed with it — where before, a
                // gear change that tore the unit's visual down left its cloud in the air.
                EffectHost { parent: Some(unit) },
                // A kit effect IS a `CEffect`: it runs the stage's animation lifecycle.
                Some(inst.stage),
                &mut FxMaterials {
                    store: &mut wow_materials,
                    tint: &mut tint_reg,
                    uv: &mut uv_reg,
                    table: &mut anim_table,
                },
                &ibps,
                &mut palettes,
                None, // an attach-point kit effect opens on its model's own `Stand`
            );
            inst.root = Some(root);
            if !inst.persistent {
                // The client's completion-callback moment: one full pass of the sequence the
                // instance arms — sequence 0, whether or not it moves a bone. Read from the raw
                // sequence table, NOT the built clips: a sequence with zero bone keys builds no
                // clip at all (the eat/drink tankard/sparkle models are exactly this), and the
                // old clips-only read collapsed their span to the 1 s fallback — the jug lived
                // 1 s of every 5 s kit resend instead of overlapping seamlessly. INFERRED at the
                // loop boundary: a looping sequence is counted as one pass (a stage-0 effect that
                // never completed would outlive the drink; the ref's tankard does not).
                let span = dm.first_seq_span.unwrap_or(FALLBACK_SPAN);
                inst.expires = Some(now + span);
            }
            if benilla_assets::trace::enabled() {
                benilla_assets::trace::line(
                    "fx",
                    &format!(
                        "kit spawn unit={unit} path={} persistent={} span={:?}",
                        inst.path,
                        inst.persistent,
                        inst.expires.map(|e| e - now)
                    ),
                );
            }
            true
        });
    }
}

/// Fire the event keyframes each live effect instance's playing clip crossed since last frame
/// (module doc) — the same `(prev, cur]` window scan as the creature scanner
/// ([`scan_events`]), emitting with the HOST UNIT as the event entity (the unit's transform is
/// the world position; an instance root's own is joint-local). Unlike a streamed creature, an
/// instance is born under our eyes at t = 0, so first sight fires the head window `[0, cur]` —
/// the level-up pillar's `$SND(888)` sits at 0.033 s and depends on it.
pub(super) fn fire_fx_anim_events(
    units: Query<(Entity, &FxAttached)>,
    players: Query<&AnimationPlayer>,
    // The instance root's own world frame and rig — the effect MODEL's placement, which is the
    // frame its event records are authored in. Decoupled from the emit entity below on purpose.
    globals: Query<&GlobalTransform>,
    poses: Query<&benilla_world::rig_anim::RigPose>,
    fx: Option<Res<SpellFx>>,
    mut last: Local<EntityHashMap<f32>>,
    mut seen: Local<Vec<Entity>>,
    mut out: MessageWriter<AnimSoundEvent>,
) {
    let Some(fx) = fx else { return };
    seen.clear();
    for (unit, att) in &units {
        for inst in &att.instances {
            let Some(root) = inst.root else { continue };
            let Some(clip) = fx
                .models
                .get(&inst.path)
                .and_then(|dm| dm.animations.as_ref())
                .and_then(|a| a.clips.first())
                .filter(|c| !c.events.is_empty())
            else {
                continue;
            };
            let Ok(player) = players.get(root) else {
                continue;
            };
            let Some(active) = player.animation(clip.node) else {
                continue;
            };
            let cur = active.seek_time();
            seen.push(root);
            let prev = last.insert(root, cur).unwrap_or(-1.0);
            // The emit entity is decoupled from the scanned track's owner by design — the fx
            // scan fires at the unit, not the joint-local instance root. **The POINT is not**:
            // the record's `position` is authored in the effect model's own space, so it composes
            // through the instance root's world frame. `Spells\ArcaneShot_Area.m2` puts its
            // `$SND` 30.5 yd off that model's origin — the largest `$SND` offset in the corpus.
            let Ok(root_world) = globals.get(root) else {
                continue;
            };
            let frame = crate::creature_anim::EventFrame {
                world: root_world,
                rig: poses
                    .get(root)
                    .ok()
                    .and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
            };
            scan_events(clip, unit, prev, cur, &frame, &mut out);
        }
    }
    // Roots despawn on reap/expiry — drop their seek memory with them.
    if last.len() > seen.len() {
        let live: bevy::ecs::entity::EntityHashSet = seen.iter().copied().collect();
        last.retain(|e, _| live.contains(e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::mesh::skinning::SkinnedMeshInverseBindposes;

    /// One recorded instance, spawned (`root`) and awaiting nothing.
    fn instance(root: Entity, persistent: bool) -> FxInstance {
        FxInstance {
            spell_id: 13033, // Ice Barrier
            persistent,
            class: FxClass::AuraState,
            stage: FxStage::State,
            tag: 0x13,
            effect: 1499, // Ice Barrier's state model
            path: "Spells\\IceShield_State.mdx".into(),
            root: Some(root),
            expires: None,
            decaying: false,
        }
    }

    /// An app with just what `attach_spell_fx` reads, plus one unit carrying an instance per
    /// entry of `persistent` — each with a freshly spawned root. Returns the app, the unit and the
    /// roots in order.
    fn standing(persistent: &[bool]) -> (App, Entity, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WowModelMaterial>()
            .init_asset::<benilla_assets::M2Model>()
            .init_asset::<SkinnedMeshInverseBindposes>()
            .init_resource::<SpellFx>()
            .init_resource::<FxTintAnims>()
            .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
            .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
            .init_resource::<benilla_world::rig_palette::RigPalettes>()
            .add_systems(Update, attach_spell_fx);
        let roots: Vec<Entity> = persistent
            .iter()
            .map(|_| app.world_mut().spawn_empty().id())
            .collect();
        let instances = roots
            .iter()
            .zip(persistent)
            .map(|(&root, &p)| instance(root, p))
            .collect();
        let unit = app
            .world_mut()
            .spawn((
                Transform::default(),
                GlobalTransform::default(),
                FxAttached { instances },
            ))
            .id();
        (app, unit, roots)
    }

    /// Each surviving instance's `(persistent, root)` after one pass.
    fn instances_of(app: &App, unit: Entity) -> Vec<(bool, Option<Entity>)> {
        app.world()
            .entity(unit)
            .get::<FxAttached>()
            .unwrap()
            .instances
            .iter()
            .map(|i| (i.persistent, i.root))
            .collect()
    }

    /// **The drain-then-recreate law** (0835's named-open item closed; the reference's rebuild
    /// `0x60abe0`): the rebuild drains EVERY effect node and then
    /// re-creates what must persist. A teardown of the unit's visual leaves each instance
    /// holding a dangling root; the persistent instance re-arms (our re-create leg — the
    /// edge-driven aura watcher cannot notice, the aura never left the slots), the one-shot
    /// flash goes exactly as the reference's drained Mount Poof does. The shapeshift cloud is
    /// deliberately NOT this path's business — it is the morph latch's post-rebuild replay
    /// (`creature_anim::spell_visual::replay_morph_kit`).
    #[test]
    fn a_persistent_instance_whose_root_died_as_collateral_is_re_armed() {
        let (mut app, unit, roots) = standing(&[true, false]);
        // The teardown: a display swap despawns the unit's whole visual under them.
        for root in roots {
            app.world_mut().entity_mut(root).despawn();
        }
        app.update();
        assert_eq!(
            instances_of(&app, unit),
            vec![(true, None)],
            "the aura's instance re-arms for the spawn pass; the one-shot drains",
        );
    }

    /// **The re-create waits for the new body.** A display swap tears the visual down and the
    /// new model may still be loading when the effect model is already cached: re-arming then
    /// found no attach points and parked a chest shield at the feet, ground-decaled, for the
    /// aura's whole life (nothing re-seats a live root). It waits until the body is built.
    #[test]
    fn a_re_armed_instance_waits_for_the_body_before_it_spawns() {
        let (mut app, unit, roots) = standing(&[true]);
        app.world_mut().entity_mut(roots[0]).despawn();
        app.world_mut().resource_mut::<SpellFx>().models.insert(
            "Spells\\IceShield_State.mdx".into(),
            DisplayModel {
                parts: Some(Vec::new()),
                ..crate::entities::display::empty_shell()
            },
        );
        app.update();
        app.update();
        assert_eq!(
            instances_of(&app, unit),
            vec![(true, None)],
            "no body yet: the instance holds, it does not spawn at the root"
        );
        app.world_mut()
            .entity_mut(unit)
            .insert(crate::entities::VisualAttached);
        app.update();
        assert!(
            matches!(instances_of(&app, unit)[..], [(true, Some(_))]),
            "the body is built: the instance spawns"
        );
    }

    /// A decaying persistent instance was already ending — the drain takes it too (a re-spawn
    /// would replay its death rattle from nothing on the new body).
    #[test]
    fn a_decaying_instance_whose_root_died_as_collateral_is_dropped() {
        let (mut app, unit, roots) = standing(&[true]);
        app.world_mut()
            .entity_mut(unit)
            .get_mut::<FxAttached>()
            .unwrap()
            .instances[0]
            .decaying = true;
        app.world_mut().entity_mut(roots[0]).despawn();
        app.update();
        assert_eq!(instances_of(&app, unit), vec![]);
    }

    /// A live root is left strictly alone — the gate is "is it still there?", not "re-spawn me".
    #[test]
    fn a_live_root_is_never_disturbed() {
        let (mut app, unit, roots) = standing(&[true]);
        app.update();
        assert_eq!(instances_of(&app, unit), vec![(true, Some(roots[0]))]);
    }

    // ---- the same-slot replace (`0x6208e0`) ----

    /// The blood spurt's record and the two flank tags — the case the rule exists for.
    const SPURT: u32 = 63;
    const FRONT: u16 = 0xf;
    const BACK: u16 = 0x10;

    /// An app running `resolve_spell_fx` alone, plus one bare unit.
    fn resolving() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WowModelMaterial>()
            .init_asset::<benilla_assets::M2Model>()
            .init_resource::<SpellFx>()
            .add_message::<SpellKitFx>()
            .add_systems(Update, resolve_spell_fx);
        let unit = app.world_mut().spawn_empty().id();
        (app, unit)
    }

    /// One self-terminating `Begin` carrying one slot.
    fn begin(entity: Entity, effect: u32, tag: u16, path: &str) -> SpellKitFx {
        SpellKitFx::Begin {
            entity,
            spell_id: 0,
            persistent: false,
            class: FxClass::Hold,
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag,
                effect,
                path: path.into(),
            }],
        }
    }

    fn send(app: &mut App, msg: SpellKitFx) {
        app.world_mut()
            .resource_mut::<Messages<SpellKitFx>>()
            .write(msg);
        app.update();
    }

    /// Every live instance's `(effect, tag)`, in list order.
    fn slots_of(app: &App, unit: Entity) -> Vec<(u32, u16)> {
        app.world()
            .entity(unit)
            .get::<FxAttached>()
            .map(|a| a.instances.iter().map(|i| (i.effect, i.tag)).collect())
            .unwrap_or_default()
    }

    /// **The rule** (`0x6208e0`): a second play of the same record at the same tag DESTROYS the
    /// first — five attackers' spurts on one flank are one instance, not five, which is why the
    /// reference's effect count is flat in the number of attackers where ours was linear.
    #[test]
    fn a_replay_of_the_same_record_at_the_same_tag_replaces_rather_than_stacks() {
        let (mut app, unit) = resolving();
        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        // The first instance has been attached: give it the root the spawn pass would.
        let root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(unit)
            .get_mut::<FxAttached>()
            .unwrap()
            .instances[0]
            .root = Some(root);

        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        assert_eq!(
            slots_of(&app, unit),
            vec![(SPURT, FRONT)],
            "one instance, not two"
        );
        assert!(
            app.world().get_entity(root).is_err(),
            "the replaced node is DESTROYED the same frame, not left to decay",
        );
    }

    /// The tag is half the key: the same record at a different attach point coexists — Execute's
    /// cast kit hangs one record on BOTH hands (`0x15` and `0x16`), and both must survive.
    #[test]
    fn the_same_record_at_a_different_tag_coexists() {
        let (mut app, unit) = resolving();
        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        send(&mut app, begin(unit, SPURT, BACK, "Particles\\Spurt.mdx"));
        assert_eq!(slots_of(&app, unit), vec![(SPURT, FRONT), (SPURT, BACK)]);
    }

    /// The record is the other half: two different records at one tag coexist, and keying the
    /// walk on the model PATH instead would wrongly collapse them (two rows may name one `.mdx`).
    #[test]
    fn a_different_record_at_the_same_tag_coexists() {
        let (mut app, unit) = resolving();
        send(&mut app, begin(unit, SPURT, FRONT, "Particles\\Spurt.mdx"));
        send(
            &mut app,
            begin(unit, SPURT + 1, FRONT, "Particles\\Spurt.mdx"),
        );
        assert_eq!(
            slots_of(&app, unit),
            vec![(SPURT, FRONT), (SPURT + 1, FRONT)]
        );
    }

    /// The reference excludes `tag == -1` from the walk (`0x620913`), so successive world plants
    /// genuinely stack — a second Thunder Clap ring does not eat the first.
    #[test]
    fn the_world_plant_slot_is_excluded_from_the_replace() {
        let (mut app, unit) = resolving();
        let tag = benilla_formats::WORLD_EFFECT_TAG;
        send(&mut app, begin(unit, SPURT, tag, "Spells\\Ring.mdx"));
        send(&mut app, begin(unit, SPURT, tag, "Spells\\Ring.mdx"));
        assert_eq!(slots_of(&app, unit), vec![(SPURT, tag), (SPURT, tag)]);
    }

    /// The stun swirl that never appeared: a kit instance whose cache entry was
    /// wiped by the login `MapChange` between its `Begin` and its spawn pass.
    ///
    /// The fixture is the aftermath, not the race — an instance holding a path the cache no longer
    /// knows, which is what a session that entered the world already carrying an aura looks like
    /// ~0.4–1.7 s in. Before [`ensure_model`] guarded the read, the spawn pass returned "still
    /// pending" for ever: nothing re-created the entry (only a fresh `SpellKitFx::Begin` inserts,
    /// and a standing aura emits no second one), and a *persistent* instance is exempt from the
    /// `PENDING_TIMEOUT` reaper, so it was never spawned, never expired and never traced.
    ///
    /// The assertions are the two halves of that: the entry comes back, **and** the instance is
    /// still there to use it — reaping it instead would make the failure quiet rather than fixed.
    #[test]
    fn an_evicted_cache_entry_is_rebuilt_by_the_spawn_pass() {
        const PATH: &str = "Spells\\StunSwirl_State_Head.mdx";

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins.build().disable::<bevy::time::TimePlugin>(),
            AssetPlugin::default(),
        ))
        .init_asset::<WowModelMaterial>()
        .init_asset::<benilla_assets::M2Model>()
        .init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>()
        .init_resource::<Time>()
        .init_resource::<SpellFx>()
        .init_resource::<FxTintAnims>()
        .init_resource::<benilla_world::doodad_anim::UvAnimMaterials>()
        .init_resource::<benilla_world::mat_anim_table::MatAnimTable>()
        .init_resource::<benilla_world::rig_palette::RigPalettes>()
        .add_systems(Update, attach_spell_fx);

        let unit = app
            .world_mut()
            .spawn((
                GlobalTransform::default(),
                FxAttached {
                    instances: vec![FxInstance {
                        spell_id: 9032,
                        persistent: true,
                        class: FxClass::AuraState,
                        stage: FxStage::State,
                        tag: 0x14,
                        effect: 50,
                        path: PATH.to_string(),
                        root: None,
                        expires: None,
                        decaying: false,
                    }],
                },
            ))
            .id();
        assert!(
            app.world().resource::<SpellFx>().models.is_empty(),
            "the fixture is a cache the eviction already cleared"
        );

        app.update();

        assert!(
            app.world().resource::<SpellFx>().models.contains_key(PATH),
            "the spawn pass must get-or-insert its entry, not read and give up"
        );
        assert_eq!(
            app.world()
                .entity(unit)
                .get::<FxAttached>()
                .expect("the unit keeps its instance list")
                .instances
                .len(),
            1,
            "the instance must survive to use the rebuilt entry — a persistent one is never reaped \
             for being pending"
        );
    }
}
