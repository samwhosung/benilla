//! **Dest-anchored spell effects** (decision 0797, B132's second half): what a ground cast shows
//! at the point. Two producers, one spawn/attach lane:
//!
//! - **The DynamicObject machine** ([`arm_ground_effects`]): a TYPEID-6 create is the anchor a
//!   persistent area effect hangs on (Blizzard's storm, Flamestrike's burn). The reference
//!   builds **two disjoint visuals**, never through the unit-kit pipeline (none of the 15
//!   `PlaySpellVisualKit` `0x60edf0` call sites is on this class):
//!   - **Visual A** — the object's own `.mdx`: SPELLID → `Spell.dbc` SpellVisual →
//!     `SpellVisual` field 11 ≠ 0 gate → field 12 → `SpellVisualEffectName` field 2 path
//!     (`0x5d57c0`), instanced at the object's position **verbatim** (no terrain projection),
//!     Z-rotated by `DYNAMICOBJECT_FACING` (`0x613ef0` → `0x7bdd60`), looping its sequence
//!     (`0x5d5580` re-fires on completion).
//!   - **Visual B** — the shard emitter (`AUBlizzardObject`, `0x5d55c0` → `0x6ece30`):
//!     `SpellVisual` field 13 → `SpellVisualKit` → the first `CharProcType == 9` block; its
//!     `CharParamZero` small-int-decodes (`bits(p0 + 512.0) >> 14 & 0xff`) into a **hardcoded
//!     7-entry model table** (`0x870e24` — [`SHARD_MODELS`]), `CharParamOne` is the emit rate
//!     (× the graphics-quality factor — we run max quality, ×1.0). Each emitted shard spawns at
//!     a random offset inside `DYNAMICOBJECT_RADIUS` — the wire radius IS the visual footprint
//!     (`0x6ebad0`). The kit's field-13 `SoundEntries` is the looping area sound.
//! - **The GO dest one-shot** ([`spawn_ground_bursts`] ← the router's [`GroundBurst`]):
//!   `SMSG_SPELL_GO` itself plays the field-12 model ONCE at the packet's dest point when
//!   `SpellVisual` field 6 == 0 (no missile owns the arrival) — `0x6e8088`–`0x6e8143`, a
//!   self-terminating CEffect. Flamestrike's initial burst; it fires **before** the dynobj
//!   create arrives and must not wait for it (a trap).
//!
//! **Teardown is a tail, not a snap** (a trap): `SMSG_DESTROY_OBJECT` despawns the anchor —
//! visual A and the emitter die with it (the ref zeroes the emit rate at `0x6ecf20`) — but the
//! already-emitted shards are FREE entities that run out their own one-pass lifetimes, exactly
//! the ref's "spawned particles finish". The looping sound dies via the sound module's
//! `RemovedComponents<NetEntity>` hold-loop reaper.
//!
//! Named approximations: the ref's 3 s sound fade at destroy is a hard stop here (no channel
//! fade affordance yet); the hardcoded loop sequence id `0x9e` (fired when `obj+0x190` bit 1 is
//! set — the bit's source is unpinned) is approximated by looping the model's first clip; the
//! quality-tier emit factor is pinned at ×1.0 (max); Flamestrike's kit sound plays even though
//! its kit has no type-9 proc (the ref couples sound to the emitter object — whether a
//! procless kit still sounds is unread; the burn crackle missing would be the wronger look).

use bevy::prelude::*;

use crate::creature_anim::{SpellKitSound, SpellVisuals};
use crate::net::{NetEntity, ObjectStore};
use benilla_protocol::EntityKind;

use super::spell_fx::{attach_effect_visuals, ensure_model, FxMaterials, SpellFx};

/// The client's hardcoded shard-model table (`0x870e24`, 7 entries). `CharParamZero`'s decoded
/// small int indexes it.
const SHARD_MODELS: [&str; 7] = [
    "Spells\\Blizzard_Impact_Base.mdx",
    "Spells\\RainOfFire_Impact_Base.mdx",
    "Spells\\CallLightning_Impact.mdx",
    "Spells\\FlamestrikeSmall_Impact_Base.mdx",
    "Spells\\DeathAndDecay_Area_Base.mdx",
    "Spells\\ArcaneShot_Area.mdx",
    "Spells\\StarShards_Impact_Base.mdx",
];

/// The `CharProcType` the dynobj emitter chain scans for (`0x5d55c0`). One key out of the same
/// dispatch space the aura-state body procs come from (`crate::aura_visual`, decision 0806) — the
/// kit column is shared, the consumers are not.
const PROC_TYPE_SHARD_EMITTER: i32 = 9;

/// The exact small-int decode the client applies to `CharParamZero`
/// ([`benilla_formats::char_proc_small_int`] — the one idiom every integer-in-a-float-column proc
/// uses, the chain proc included; decision 0955 lifted it into the format crate) — clamped into
/// [`SHARD_MODELS`] because the client itself has **no bounds check** (`mov cl,al` in `0x5d55c0`
/// — data ≥ 7 reads past the table; a trap).
fn shard_model_index(param0: f32) -> usize {
    let idx = benilla_formats::char_proc_small_int(param0) as usize;
    idx.min(SHARD_MODELS.len() - 1)
}

/// A free-standing dest-anchored effect-model instance (visual A, a shard, a GO burst) —
/// [`attach_ground_fx_models`] hangs the model parts once the M2 builds.
#[derive(Component)]
pub(super) struct GroundFx {
    /// The [`SpellFx`] model-cache key.
    path: String,
    /// `true` = loop the clip for the instance's whole life (visual A — the ref re-fires its
    /// sequence on completion); `false` = one pass of sequence 0 then despawn (a shard, a
    /// burst — the kit pipeline's self-termination clock).
    looping: bool,
    /// Parts attached (the model was ready).
    spawned: bool,
    /// The repeat override landed on the live `AnimationPlayer` (loop instances only).
    loop_armed: bool,
    /// One-shot self-termination deadline (`time.elapsed_secs()`), set at attach.
    expires: Option<f32>,
}

impl GroundFx {
    fn new(path: String, looping: bool) -> Self {
        Self {
            path,
            looping,
            spawned: false,
            loop_armed: false,
            expires: None,
        }
    }
}

/// The shard emitter riding a DynamicObject anchor (visual B — the ref's `AUBlizzardObject`).
/// Dies with the anchor (emission stops at destroy); its spawned shards are free entities and
/// finish on their own — the ref's emitter tail.
#[derive(Component)]
pub(super) struct ShardEmitter {
    /// The [`SHARD_MODELS`] path.
    path: String,
    /// `DYNAMICOBJECT_RADIUS` — the per-shard random spawn spread (the AoE footprint).
    radius: f32,
    /// Shards per second (`CharParamOne` × the quality factor, ×1.0 here).
    rate: f32,
    /// Fractional emissions carried between frames.
    accum: f32,
    /// A tiny LCG for the spawn offsets (the ref rolls real randoms; seeded per emitter).
    rng: u64,
}

/// The router's dest one-shot order (`SMSG_SPELL_GO` + `TARGET_FLAG_DEST_LOCATION`, gate
/// `SpellVisual` field 6 == 0 ∧ field 12 ≠ 0 — resolved router-side where the catalogs live).
#[derive(Message, Clone)]
pub(crate) struct GroundBurst {
    /// The field-12 `SpellVisualEffectName` model path.
    pub(crate) path: String,
    /// The dest point, bevy coords (converted at the apply seam).
    pub(crate) pos: Vec3,
}

/// Arm a freshly-created DynamicObject anchor: resolve the spell's visual row and hang the two
/// visuals + the area sound (module doc). Runs on `Added<ObjectStore>` so the create's fields
/// are the trigger; a dynobj is created once and never re-created in place (a re-cast is a new
/// guid — captured live, 0797).
pub(super) fn arm_ground_effects(
    mut commands: Commands,
    created: Query<(Entity, &NetEntity, &ObjectStore), Added<ObjectStore>>,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    mut sounds: MessageWriter<SpellKitSound>,
) {
    let (Some(visuals), Some(spells), Some(mut fx)) = (visuals, spells, fx) else {
        return;
    };
    for (anchor, net, store) in &created {
        if net.kind != EntityKind::DynamicObject {
            continue;
        }
        let Some(spell_id) = store.0.dynamicobject_spell_id() else {
            continue;
        };
        let Some(stages) = spells
            .catalog
            .get(spell_id)
            .and_then(|d| visuals.0.stages(d.visual))
        else {
            debug!("dest_fx: dynobj spell {spell_id} has no visual row — invisible area");
            continue;
        };
        let facing = store
            .0
            .dynamicobject_position()
            .map(|(_, f)| f)
            .unwrap_or(0.0);
        // Visual A — the object's own model, gated on field 11, Z-rotated by FACING. A child of
        // the anchor: the destroy pop takes it exactly like the ref's scene-node teardown.
        if stages.area_gate != 0 && stages.area_effect != 0 {
            if let Some(path) = visuals.0.effect_path(stages.area_effect) {
                let path = path.to_string();
                ensure_model(&mut fx, &asset_server, &path);
                let child = commands
                    .spawn((
                        GroundFx::new(path, true),
                        Transform::from_rotation(Quat::from_rotation_y(facing)),
                        Visibility::default(),
                    ))
                    .id();
                commands.entity(anchor).add_child(child);
            }
        }
        // Visual B — the shard emitter, from the area kit's first type-9 CharProc block; the
        // kit's own sound column is the looping area sound (a tracked hold loop the sound
        // module reaps when the anchor's NetEntity is removed).
        if let Some(kit) = visuals.0.kit(stages.area_kit) {
            if let Some(proc) = kit.char_procs().find(|p| p.ty == PROC_TYPE_SHARD_EMITTER) {
                let path = SHARD_MODELS[shard_model_index(proc.params[0])].to_string();
                ensure_model(&mut fx, &asset_server, &path);
                commands.entity(anchor).insert(ShardEmitter {
                    path,
                    radius: store.0.dynamicobject_radius().unwrap_or(0.0),
                    rate: proc.params[1],
                    accum: 0.0,
                    rng: 0x9e3779b97f4a7c15 ^ anchor.to_bits(),
                });
            }
            if let Some(kit_sound) = kit.sound {
                sounds.write(SpellKitSound::Play {
                    entity: anchor,
                    kit_sound,
                });
            }
        }
        debug!(
            "dest_fx: dynobj armed — spell {spell_id}, gateA={} effect={} kit={} radius {:?}",
            stages.area_gate,
            stages.area_effect,
            stages.area_kit,
            store.0.dynamicobject_radius(),
        );
    }
}

/// Spawn the router's GO dest one-shots — free entities at the packet's point, one sequence
/// pass, then gone. Fired at the GO, never waiting on the dynobj create (a trap).
pub(super) fn spawn_ground_bursts(
    mut commands: Commands,
    mut bursts: MessageReader<GroundBurst>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
) {
    let Some(mut fx) = fx else { return };
    for burst in bursts.read() {
        ensure_model(&mut fx, &asset_server, &burst.path);
        commands.spawn((
            GroundFx::new(burst.path.clone(), false),
            Transform::from_translation(burst.pos),
            Visibility::default(),
        ));
    }
}

/// Emit shards: `rate` per second, each a free one-shot instance at a random offset inside the
/// wire radius (uniform over the disc, horizontal plane — the ref's per-particle spread scaled
/// by `+0x11c`).
pub(super) fn tick_shard_emitters(
    mut commands: Commands,
    time: Res<Time>,
    mut emitters: Query<(&mut ShardEmitter, &GlobalTransform)>,
) {
    for (mut em, tf) in &mut emitters {
        em.accum += em.rate * time.delta_secs();
        while em.accum >= 1.0 {
            em.accum -= 1.0;
            // xorshift* — cheap, per-emitter deterministic, no rand dependency.
            let mut next = || {
                em.rng ^= em.rng >> 12;
                em.rng ^= em.rng << 25;
                em.rng ^= em.rng >> 27;
                em.rng.wrapping_mul(0x2545F4914F6CDD1D)
            };
            let u1 = (next() >> 40) as f32 / (1u64 << 24) as f32;
            let u2 = (next() >> 40) as f32 / (1u64 << 24) as f32;
            let r = em.radius * u1.sqrt();
            let theta = u2 * std::f32::consts::TAU;
            let offset = Vec3::new(r * theta.cos(), 0.0, r * theta.sin());
            commands.spawn((
                GroundFx::new(em.path.clone(), false),
                Transform::from_translation(tf.translation() + offset),
                Visibility::default(),
            ));
        }
    }
}

/// Attach model parts to pending instances whose M2 finished building (the missile pattern —
/// free world models, ground-anchored so authored flat quads decal to the terrain), start the
/// one-shot clocks, and run both reapers (one-shot expiry; the loop-repeat override).
pub(super) fn attach_ground_fx_models(
    mut commands: Commands,
    time: Res<Time>,
    mut instances: Query<(Entity, &mut GroundFx, Option<&mut AnimationPlayer>)>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    mut wow_materials: ResMut<Assets<benilla_assets::materials::WowModelMaterial>>,
    mut tint_reg: ResMut<super::spell_fx::FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
) {
    let Some(mut fx) = fx else { return };
    let now = time.elapsed_secs();
    for (entity, mut inst, player) in &mut instances {
        if !inst.spawned {
            ensure_model(&mut fx, &asset_server, &inst.path);
            let Some(dm) = fx.models.get(&inst.path) else {
                continue; // unreachable — just inserted
            };
            if !attach_effect_visuals(
                &mut commands,
                entity,
                dm,
                now,
                true, // a dest-anchored area model's flat quads ARE ground decals
                // A free world model standing at the point, chained to nothing: its trail stays
                // world-frozen and its pool finishes in place when the effect ends.
                super::spell_fx::EffectHost::default(),
                // The dest one-shot is not a `CEffect` on a unit: it plants at the packet's point and
                // runs its own span clock, so it keeps the plain single-clip arm.
                None,
                &mut FxMaterials {
                    store: &mut wow_materials,
                    tint: &mut tint_reg,
                    uv: &mut uv_reg,
                    table: &mut anim_table,
                },
                &ibps,
                &mut palettes,
                None,
            ) {
                continue; // model still building — attach on a later pass
            }
            inst.spawned = true;
            if !inst.looping {
                // One pass of the first sequence — the kit pipeline's completion-callback
                // stand-in (`spell_fx`'s span clock, same law).
                let span = dm.first_seq_span.unwrap_or(super::spell_fx::FALLBACK_SPAN);
                inst.expires = Some(now + span);
            }
            continue; // the AnimationPlayer lands next frame — the loop override waits
        }
        // The loop override: the ref re-fires the sequence on completion (`0x5d5580`); ours
        // marks the live player's animations repeat-forever, once.
        if inst.looping && !inst.loop_armed {
            if let Some(mut player) = player {
                for (_, anim) in player.playing_animations_mut() {
                    anim.repeat();
                }
                inst.loop_armed = true;
            }
        }
        if let Some(expires) = inst.expires {
            if now >= expires {
                commands.entity(entity).despawn();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `0x5d55c0` decode: `bits(f32(param0 + 512.0)) >> 14 & 0xff` recovers the small int
    /// exactly (the real rows carry 0.0 → Blizzard, 1.0 → Rain of Fire), and out-of-table data
    /// clamps instead of reading past the 7 entries (the client's own missing bounds check — a
    /// trap).
    #[test]
    fn shard_model_index_decodes_and_clamps() {
        assert_eq!(shard_model_index(0.0), 0);
        assert_eq!(shard_model_index(1.0), 1);
        assert_eq!(shard_model_index(6.0), 6);
        assert_eq!(shard_model_index(7.0), 6, "clamped, not read past");
        assert_eq!(shard_model_index(200.0), 6);
        assert_eq!(SHARD_MODELS[0], "Spells\\Blizzard_Impact_Base.mdx");
        assert_eq!(SHARD_MODELS[1], "Spells\\RainOfFire_Impact_Base.mdx");
    }
}
