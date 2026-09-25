//! Dest-anchored spell effects: what a ground cast shows at its point. A DynamicObject anchors a
//! persistent area effect with two visuals, its own looping model and a shard emitter, outside the
//! unit kit pipeline (no `PlaySpellVisualKit` `0x60edf0` call site is on this class), and
//! `SMSG_SPELL_GO` plays a one-shot at the dest point.

use bevy::prelude::*;

use crate::creature_anim::{SpellKitSound, SpellVisuals};
use crate::net::{NetEntity, ObjectStore};
use benilla_protocol::EntityKind;

use super::spell_fx::{attach_effect_visuals, ensure_model, FxMaterials, SpellFx};

/// The client's hardcoded shard-model table (`0x870e24`), indexed by `CharParamZero`'s small int.
const SHARD_MODELS: [&str; 7] = [
    "Spells\\Blizzard_Impact_Base.mdx",
    "Spells\\RainOfFire_Impact_Base.mdx",
    "Spells\\CallLightning_Impact.mdx",
    "Spells\\FlamestrikeSmall_Impact_Base.mdx",
    "Spells\\DeathAndDecay_Area_Base.mdx",
    "Spells\\ArcaneShot_Area.mdx",
    "Spells\\StarShards_Impact_Base.mdx",
];

/// The `CharProcType` whose first kit block the emitter chain takes (`0x5d55c0`).
const PROC_TYPE_SHARD_EMITTER: i32 = 9;

/// `CharParamZero`'s small int ([`benilla_formats::char_proc_small_int`]), clamped into
/// [`SHARD_MODELS`]: the client has no bounds check (`mov cl,al` in `0x5d55c0`) and reads past it.
fn shard_model_index(param0: f32) -> usize {
    let idx = benilla_formats::char_proc_small_int(param0) as usize;
    idx.min(SHARD_MODELS.len() - 1)
}

/// A dest-anchored effect model: visual A, a shard or a GO burst.
#[derive(Component)]
pub(super) struct GroundFx {
    /// The [`SpellFx`] model-cache key.
    path: String,
    /// Loop for life (visual A), or one pass of sequence 0 then despawn (a shard, a burst).
    looping: bool,
    /// Parts attached (the model was ready).
    spawned: bool,
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

/// Visual B, a DynamicObject's shard emitter (`AUBlizzardObject`, `0x5d55c0`, `0x6ece30`). It dies
/// with the anchor (`0x6ecf20` zeroes the rate); its shards are free entities that run out their
/// own lifetimes.
#[derive(Component)]
pub(super) struct ShardEmitter {
    /// The [`SHARD_MODELS`] path.
    path: String,
    /// `DYNAMICOBJECT_RADIUS`: the wire radius is the spread (`0x6ebad0`).
    radius: f32,
    /// Shards per second: `CharParamOne` times the graphics-quality factor, here its 1.0 maximum.
    rate: f32,
    /// Fractional emissions carried between frames.
    accum: f32,
    /// xorshift* state for the spawn offsets, seeded per emitter: no rand dependency.
    rng: u64,
}

/// The router's dest one-shot: `SMSG_SPELL_GO` to a dest location plays `SpellVisual` field 12 once
/// at the point when field 6, the missile, is 0 (`0x6e8088`..`0x6e8143`).
#[derive(Message, Clone)]
pub(crate) struct GroundBurst {
    /// The field-12 `SpellVisualEffectName` model path.
    pub(crate) path: String,
    /// The dest point, in Bevy coordinates.
    pub(crate) pos: Vec3,
}

/// Arm each new DynamicObject off its create's fields: visual A, the shard emitter and the area
/// sound. A re-cast is a new guid, never a re-create in place.
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
        // Visual A (`0x5d57c0`): field 12's model when field 11 is set, at the object's position
        // with no terrain projection, turned by `DYNAMICOBJECT_FACING` (`0x613ef0`, `0x7bdd60`);
        // as a child it goes with the anchor.
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
        // Visual B from field 13's kit, whose sound loops as the area sound: stopped at destroy,
        // where the reference fades it over 3 s. A kit with no type-9 block sounds here; whether
        // the reference, which ties the sound to the emitter, sounds it is untraced.
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

/// Spawn the GO dest one-shots at once, never waiting on the DynamicObject create that follows.
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

/// Emit `rate` shards per second, each a free one-shot at a uniform random point of the radius's
/// horizontal disc (the reference's spread scaled by `+0x11c`).
pub(super) fn tick_shard_emitters(
    mut commands: Commands,
    time: Res<Time>,
    mut emitters: Query<(&mut ShardEmitter, &GlobalTransform)>,
) {
    for (mut em, tf) in &mut emitters {
        em.accum += em.rate * time.delta_secs();
        while em.accum >= 1.0 {
            em.accum -= 1.0;
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

/// Attach each pending instance's parts once its M2 builds, start the one-shot clocks, arm the
/// loops and despawn expired one-shots.
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
                continue; // unreachable: just inserted
            };
            if !attach_effect_visuals(
                &mut commands,
                entity,
                dm,
                now,
                true, // a dest-anchored model's flat quads are ground decals
                // Chained to nothing: its trail stays world-frozen and its pool finishes in place.
                super::spell_fx::EffectHost::default(),
                // Not a `CEffect` on a unit: its own span clock, the plain single-clip arm.
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
                continue; // model still building
            }
            inst.spawned = true;
            if !inst.looping {
                // One pass of the first sequence, as `spell_fx`'s span clock times a kit.
                let span = dm.first_seq_span.unwrap_or(super::spell_fx::FALLBACK_SPAN);
                inst.expires = Some(now + span);
            }
            continue; // the AnimationPlayer lands next frame
        }
        // The reference re-fires on completion (`0x5d5580`), the hardcoded sequence `0x9e` when
        // `obj+0x190` bit 1 is set (its source untraced); this repeats the first clip.
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

    /// The `0x5d55c0` decode, `bits(f32(param0 + 512.0)) >> 14 & 0xff`; the real rows carry 0.0
    /// (Blizzard) and 1.0 (Rain of Fire).
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
