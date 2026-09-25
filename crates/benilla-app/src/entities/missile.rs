//! Spell missiles: the projectile a cast with a `Spell.dbc` Speed above 0 flies at each target, and
//! the hand-off when it arrives. The router emits one [`MissileSpawn`] per GO; this module owns
//! launch and flight.
//!
//! Launch is release-keyed: a GO whose cast kit plays a body animation parks its projectiles on
//! the caster ([`PendingMissiles`]) until the animation's `$CSL`/`$CSR`/`$CST` (casting hand) or
//! `$BWR` (ranged) event drains them from its live position (`0x5ffbd0` → `0x60c940`). A GO with no
//! cast-kit animation launches at once (`0x6e7a70`), and a queue whose event never fires launches
//! when the one-shot ends (`0x5fc920`), both from the `$CSL` → `$CSR` → `$CST` → base cascade
//! (`0x60c9b0`).
//!
//! The arrival deadline is fixed at GO: the launch (`0x61ceb0`) flies `distance / Speed` less the
//! time queued, and a release already past it, as at melee range, shows only the impact. In flight
//! the missile homes on the target's live attachment and arrives on time (`0x61e2a0`).
//!
//! On arrival `0x61e1d0` picks an arm by whether the target guid still resolves, then by the hit
//! bit `[missile+0x38] & 1`, set only at the spawn (`0x60a4e8`):
//! - `0x61dc50`, a live target hit: wound, flinch and impact visual, no word; here
//!   [`CastEventKind::Impact`].
//! - `0x61dd50`, a live target missed: the outcome word, `UNIT_COMBAT` and chat, then the
//!   disposition; here the defense clip and [`MissileMiss`].
//! - `0x61d870`, no live target: plays on the caster, then walks the missile's guid list skipping
//!   code 0; here [`CastEventKind::GroundImpact`]. A target that despawned mid-flight lands here as
//!   well as a ground cast.
//!
//! With no visual-chain model a missile flies the wire's ammo (Auto Shot, Shoot, Throw: all
//! `SpellVisual` 0), an `ItemDisplayInfo` row resolved by shape: a model in the right slot is
//! `Item\ObjectComponents\Ammo\`, in the left `…\Weapon\` (thrown), each with its object skin. The
//! reference resolves the row through `0x479f40`, forking on the wire's ammo InventoryType (`0x19`,
//! thrown), and the shape rule matches it on every shipped row. With no model at all the missile
//! flies invisible and still impacts on schedule.
//!
//! A GO with no unit targets but a point flies one projectile at it (`0x6e8a50`'s empty-hit-array
//! arm calls `0x60a3d0` once, unit slot −1): the Flare, a bomb thrown at empty ground. It aims at a
//! fixed point ([`Aim::Ground`]), arrives as [`CastEventKind::GroundImpact`] on the caster
//! (`0x61d870`) and flies the same straight, arrive-on-time line as a unit missile: the
//! reference's flight is linear, with no gravity (`0x61ceb0`, `0x61e2a0`).
//!
//! Not built: `$BWR` launches from its event marker, where the reference first tries the ranged
//! weapon's HandArrow/Bullet attachment (`0x23`/`0x24`); homing aims at the attachment point, not
//! the ray-sphere intercept `0x61d230`, the same body point in practice; and `0x61dd50`'s
//! per-outcome disposition (jump table `0x61d76c`, cases `0x61d784`), which keeps a BLOCK 5000 ms
//! (`0x61e7c0`), rebuilds a RESIST or IMMUNE model (`0x707350` on `0x861790`), curves a DEFLECT
//! away (`0x4531e0`, `0x7be490`) and re-launches a REFLECT at the caster, where every miss here
//! ends on arrival.

use bevy::animation::transition::AnimationTransitions;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::creature_anim::{
    oneshot_is_live, AnimData, AnimDriver, AnimSoundEvent, CastEvent, CastEventKind, DefenseAnim,
    MissileSpawn,
};
use benilla_assets::m2_url;

use super::equipment::ItemDisplays;
use super::spell_fx::{attach_effect_visuals, ensure_model, EffectHost, FxMaterials, SpellFx};
use super::{BoneAttach, DisplayModel, ModelHandle};

/// The release events whose dispatcher arms drain the queue (`0x5ffbd0` → `0x60c940`).
const RELEASE_IDENTS: [[u8; 4]; 4] = [*b"$CSL", *b"$CSR", *b"$CST", *b"$BWR"];

/// The `0x60c9b0` launch cascade when no release event names a point, before the unit's base.
const MARKER_CASCADE: [[u8; 4]; 3] = [*b"$CSL", *b"$CSR", *b"$CST"];

/// Seconds a queue waits for a one-shot that never starts, well above the frame or two a release
/// animation takes to arm. The reference's per-animation finish callback always fires; here the end
/// is a polled edge, and a kit animation that never resolves on the model would hang the queue.
const RELEASE_WAIT_MAX: f32 = 0.25;

/// The destination attachments tried after the spawn's own tag, as the reference cascades.
const DEST_FALLBACKS: [u16; 2] = [0xf, 0x13];

/// `AnimationData.dbc` InFlight, the sequence a projectile model plays in flight: a thrown weapon
/// tumbles end over end and shows its trail only here; a model without it plays its first clip.
const INFLIGHT_ANIM: u16 = 144;

/// The missile's orientation for flight direction `f`, the reference's frame (`0x61e2a0`): model
/// +X along the flight, side = up × dir, up = dir × side, so it never rolls however the flight
/// pitches. In Bevy, local −Z (wow +X) maps to `f` and local +Y (wow +Z) to that up.
fn missile_facing(f: Vec3) -> Quat {
    let side = Vec3::Y.cross(f);
    if side.length_squared() < 1e-6 {
        // Straight up or down: any roll will do.
        return Quat::from_rotation_arc(-Vec3::Z, f);
    }
    let up = f.cross(side.normalize()).normalize();
    // Columns are the images of local X, Y, Z: `f × up` (−side, the image of wow +Y), `up`, `−f`.
    Quat::from_mat3(&Mat3::from_cols(f.cross(up), up, -f))
}

/// The projectile's looping flight sound, `SpellVisual` field 10: the reference's first flight
/// step starts it (`0x61e79e`) and every later step moves it with the missile (`0x61e77c`).
/// Written by [`spawn_missiles`] and [`move_missiles`] for `crate::sound::missile`.
#[derive(Message, Clone, Copy)]
pub(crate) enum MissileSound {
    /// Start `kit_sound` on the launched `entity`, born at `pos`, the launch point, since the new
    /// entity's `Transform` may not have flushed yet.
    Start {
        entity: Entity,
        kit_sound: u32,
        pos: Vec3,
    },
    /// The projectile arrived or streamed out.
    Stop { entity: Entity },
}

/// What a projectile flies at, the reference's two `CMissile` kinds: an owning unit slot, or −1
/// for the location fallback (`0x60a5ec`).
#[derive(Clone, Copy)]
enum Aim {
    /// A unit target, homed to its live attachment, so a moving target bends the path.
    Unit {
        target: Entity,
        /// The attachment it homes to (`SpellVisual` field 9); `None` is the base.
        dest_tag: Option<u16>,
        /// The wire's `SpellMissInfo` code for a miss, which arrives as the victim's defense clip
        /// instead of the impact; `None` for a hit.
        miss: Option<u8>,
    },
    /// The GO's destination point; arrival is the ground hand-off on the caster.
    Ground(Vec3),
}

/// One projectile in flight.
#[derive(Component)]
pub(super) struct Missile {
    spell_id: u32,
    /// A ground arrival plays on the caster (`0x61d870`), a unit arrival on the target.
    caster: Entity,
    aim: Aim,
    /// The [`SpellFx`] cache key; `None` flies invisible and still impacts.
    path: Option<String>,
    /// Remaining flight time in seconds, the arrive-on-time integrator's state.
    arrive_in: f32,
    parts_spawned: bool,
    /// The caster's ranged-weapon `SpellVisual`, resolved at GO, so a basic shot's impact kit
    /// resolves even if the caster despawns mid-flight.
    weapon_visual: Option<u32>,
}

/// The unit-side inputs of an attachment or marker position read: the base frame, the tables and
/// the pose, read through `RigPose::posed_point` without an anchor entity.
pub(super) type AttachPosQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static GlobalTransform,
        Option<&'static BoneAttach>,
        Option<&'static benilla_world::rig_anim::RigPose>,
    ),
>;

/// A unit's attachment position through the tag cascade, else its base; `None` only when the unit
/// is gone. Chain-beam endpoints use it too, as `0x6ec780` reads the same table the same way.
pub(super) fn attach_world_pos(
    unit: Entity,
    tags: impl IntoIterator<Item = u16>,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
) -> Option<Vec3> {
    let (base, bones, pose) = units.get(unit).ok()?;
    let point = bones.zip(pose).and_then(|(b, p)| {
        let (bone, offset) = tags
            .into_iter()
            .find_map(|tag| b.points.get(&tag).copied())?;
        p.posed_point(joints.get(p.joints_root).ok()?, bone, offset)
    });
    Some(point.unwrap_or_else(|| base.translation()))
}

/// This frame's aim, re-solved every tick as the reference does.
fn aim_point(aim: Aim, units: &AttachPosQuery, joints: &Query<&GlobalTransform>) -> Option<Vec3> {
    match aim {
        Aim::Unit {
            target, dest_tag, ..
        } => attach_world_pos(
            target,
            dest_tag.into_iter().chain(DEST_FALLBACKS),
            units,
            joints,
        ),
        Aim::Ground(pos) => Some(pos),
    }
}

/// The caster's launch point: the fired release event's own marker, as the dispatcher passes the
/// event's live position (`0x5ffbd0` → `0x60c940`), else the `0x60c9b0` cascade at this pose.
fn launch_world_pos(
    caster: Entity,
    fired: Option<[u8; 4]>,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
) -> Option<Vec3> {
    let (base, bones, pose) = units.get(caster).ok()?;
    let point = bones.zip(pose).and_then(|(b, p)| {
        let (bone, offset) = fired
            .into_iter()
            .chain(MARKER_CASCADE)
            .find_map(|ident| b.markers.get(&ident).copied())?;
        p.posed_point(joints.get(p.joints_root).ok()?, bone, offset)
    });
    Some(point.unwrap_or_else(|| base.translation()))
}

/// One GO's projectiles parked on the caster until the release keyframe, a node of the reference's
/// `unit+0xac` list; its model loads from queue time, as the reference creates the M2 there.
struct QueuedGo {
    spawn: MissileSpawn,
    /// The [`SpellFx`] key that becomes [`Missile::path`].
    key: Option<String>,
    /// Seconds queued, subtracted from the flight at launch (`0x61ceb0`'s `elapsed` term).
    queued: f32,
    /// A one-shot has played on the caster since queuing, which arms the anim-end flush.
    saw_oneshot: bool,
}

/// Every caster's queue of projectiles awaiting release, the reference's `CGUnit+0xac` list of
/// `CMissile` nodes, filled by the spawner `0x60a3d0` and drained by the release event; a caster
/// that streams out drops its queue. It is also a gate: the `$BWR` handler tests the list head
/// (`0x600182`) and only when a projectile waits re-animates the ranged prop, looks up the
/// attachment and launches (`0x600294` calls `0x60c940`), so a shot's flex and launch are one act.
#[derive(Resource, Default)]
pub(crate) struct PendingMissiles(EntityHashMap<Vec<QueuedGo>>);

impl PendingMissiles {
    /// Whether a projectile waits on `caster` for its release keyframe, the reference's list-head
    /// test; [`crate::ranged_flex`] asks it before the drain, in the reference's order.
    pub(crate) fn releasing(&self, caster: Entity) -> bool {
        self.0.get(&caster).is_some_and(|q| !q.is_empty())
    }

    /// Queues one projectile on `caster` for tests of the gate's consumers; only its presence
    /// matters.
    #[cfg(test)]
    pub(crate) fn queue_a_shot(app: &mut bevy::app::App, caster: Entity) {
        let spawn = MissileSpawn {
            caster,
            spell_id: 75,
            path: None,
            ammo_display_id: None,
            dest_tag: None,
            speed: 40.0,
            targets: Vec::new(),
            ground_aim: None,
            weapon_visual: None,
            missile_sound: None,
            awaits_release: true,
        };
        app.world_mut()
            .resource_mut::<Self>()
            .0
            .entry(caster)
            .or_default()
            .push(QueuedGo {
                spawn,
                key: None,
                queued: 0.0,
                saw_oneshot: false,
            });
    }
}

/// The ammo display's flight model by the shape rule (module docs), as a [`SpellFx`] entry keyed
/// `ammo:<display id>`, so two displays sharing a model but not a skin never collide.
fn ensure_ammo_model(
    fx: &mut SpellFx,
    displays: &ItemDisplays,
    display_id: u32,
    asset_server: &AssetServer,
) -> Option<String> {
    let key = format!("ammo:{display_id}");
    if !fx.models.contains_key(&key) {
        let row = displays.catalog.get(display_id)?;
        let (dir_name, col) = if row.model[0].is_some() {
            ("Weapon", 0)
        } else {
            ("Ammo", 1)
        };
        let model = row.model[col].as_ref()?;
        let dir = format!("Item\\ObjectComponents\\{dir_name}");
        let dm = DisplayModel {
            handle: ModelHandle::M2(asset_server.load(m2_url(&format!("{dir}\\{model}")))),
            object_texture: row.model_texture[col].clone(),
            dir,
            ..super::empty_shell()
        };
        fx.models.insert(key.clone(), dm);
    }
    Some(key)
}

/// A travelling spell's deferred miss word. `SMSG_SPELL_GO` skips its inline word when the
/// `Spell.dbc` Speed is nonzero (`0x6e7d4e`); the arrival re-resolves the spell from
/// `[missile+0x18]` and calls the same emitter `0x607140`. Its source class, CVar gates and colour
/// are `crate::combat_text::missile_miss_text`'s.
#[derive(Message, Clone, Copy)]
pub(crate) struct MissileMiss {
    /// The colour law's source class (`K`).
    pub(crate) caster: Entity,
    /// Always the missed target (`0x61ddb3`; `0x61d9dc`'s is the per-entry target). Unlike the
    /// inline site the arrival floats Reflect over the target, and the caster's word belongs to
    /// the re-launched flight, which is not built.
    pub(crate) anchor: Entity,
    /// The colour law's `B` bit resolves off its record.
    pub(crate) spell_id: u32,
    /// The wire's `SpellMissInfo` code (1–11).
    pub(crate) code: u8,
}

/// A travelling spell is deflected, never parried: the launch classifier `0x61d720`, called from
/// the launch at `0x61cebb`, stores PARRY(4) back as DEFLECT(9) (`0x61d756`), so the word, the
/// `UNIT_COMBAT` feed and the chat line all see 9. Every other code passes through.
fn launch_outcome_code(code: u8) -> u8 {
    if code == 4 {
        9
    } else {
        code
    }
}

/// The victim clip a missed arrival plays, as the melee `$CPP` dispatch's victim state
/// ([`DefenseAnim`]): DODGE(3) plays Dodge (30), BLOCK(5) ShieldBlock (24), any other code
/// nothing; PARRY(4) never arrives, as [`launch_outcome_code`] rewrote it.
fn miss_defense_state(code: u8) -> Option<u32> {
    match code {
        3 => Some(2), // SPELL_MISS_DODGE → the dispatch's DODGES state
        5 => Some(5), // SPELL_MISS_BLOCK → BLOCKS
        _ => None,
    }
}

/// Everything a landing projectile emits, as one system parameter: both missile systems sit at
/// Bevy's 16-parameter ceiling.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ArrivalOut<'w> {
    impacts: MessageWriter<'w, CastEvent>,
    defenses: MessageWriter<'w, DefenseAnim>,
    words: MessageWriter<'w, MissileMiss>,
}

/// The arrival hand-off, by the arms of the reference's per-tick dispatch `0x61e1d0` (module docs).
fn arrival_handoff(
    missile: &Missile,
    at: Vec3,
    out: &mut ArrivalOut,
    play_seq: &mut crate::creature_anim::PlaySeq,
) {
    // A fresh tick sorts the arrival after the frame's packet handlers, as the reference plays it.
    let mut event = |entity, kind| CastEvent {
        entity,
        spell_id: missile.spell_id,
        kind,
        seq: play_seq.next(),
    };
    match missile.aim {
        Aim::Unit {
            target, miss: None, ..
        } => {
            let kind = CastEventKind::Impact {
                weapon_visual: missile.weapon_visual,
            };
            out.impacts.write(event(target, kind));
        }
        Aim::Unit {
            target,
            miss: Some(code),
            ..
        } => {
            if let Some(victim_state) = miss_defense_state(code) {
                out.defenses.write(DefenseAnim {
                    victim: target,
                    victim_state,
                });
            }
            out.words.write(MissileMiss {
                caster: missile.caster,
                anchor: target,
                spell_id: missile.spell_id,
                code,
            });
        }
        Aim::Ground(_) => {
            out.impacts.write(event(
                missile.caster,
                CastEventKind::GroundImpact { pos: at },
            ));
        }
    }
}

/// Launch one queued GO from `launch`: a missile per target, or the one ground shot, each flying
/// `distance / speed − time queued` (`0x61ceb0`); one already past its deadline hands off at once.
fn launch_go(
    go: &QueuedGo,
    launch: Vec3,
    commands: &mut Commands,
    units: &AttachPosQuery,
    joints: &Query<&GlobalTransform>,
    sounds: &mut MessageWriter<MissileSound>,
    out: &mut ArrivalOut,
    play_seq: &mut crate::creature_anim::PlaySeq,
) {
    // The unit targets, else the location fallback's one ground shot: the reference reads the
    // `flags & 0x60` latch only for an empty hit array.
    let aims: Vec<Aim> = if go.spawn.targets.is_empty() {
        go.spawn.ground_aim.map(Aim::Ground).into_iter().collect()
    } else {
        go.spawn
            .targets
            .iter()
            .map(|&(target, miss)| Aim::Unit {
                target,
                dest_tag: go.spawn.dest_tag,
                miss: miss.map(launch_outcome_code),
            })
            .collect()
    };
    for aim_at in aims {
        let Some(aim) = aim_point(aim_at, units, joints) else {
            continue; // target already gone
        };
        let missile = Missile {
            spell_id: go.spawn.spell_id,
            caster: go.spawn.caster,
            aim: aim_at,
            path: go.key.clone(),
            arrive_in: launch.distance(aim) / go.spawn.speed.max(f32::EPSILON) - go.queued,
            // An invisible flight has no parts to wait on.
            parts_spawned: go.key.is_none(),
            weapon_visual: go.spawn.weapon_visual,
        };
        if missile.arrive_in <= 0.0 {
            arrival_handoff(&missile, aim, out, play_seq);
            continue;
        }
        let dir = (aim - launch).normalize_or(-Vec3::Z);
        let entity = commands
            .spawn((
                missile,
                Transform::from_translation(launch).with_rotation(missile_facing(dir)),
                Visibility::default(),
            ))
            .id();
        // The flight loop starts at launch and rides the projectile, even an invisible one.
        if let Some(kit_sound) = go.spawn.missile_sound {
            sounds.write(MissileSound::Start {
                entity,
                kit_sound,
                pos: launch,
            });
        }
    }
}

/// Takes the router's [`MissileSpawn`]s and runs the pending queue (module docs): park, drain on
/// the release keyframe or launch at once, and flush a queue whose keyframe never comes.
pub(super) fn spawn_missiles(
    mut commands: Commands,
    time: Res<Time>,
    mut spawns: MessageReader<MissileSpawn>,
    mut anim_events: MessageReader<AnimSoundEvent>,
    mut pending: ResMut<PendingMissiles>,
    fx: Option<ResMut<SpellFx>>,
    displays: Option<Res<ItemDisplays>>,
    asset_server: Res<AssetServer>,
    units: AttachPosQuery,
    joints: Query<&GlobalTransform>,
    casters: Query<(
        &AnimDriver,
        &AnimationPlayer,
        &AnimationTransitions,
        &benilla_assets::ModelAnimations,
    )>,
    anim_data: Option<Res<AnimData>>,
    mut sounds: MessageWriter<MissileSound>,
    mut out: ArrivalOut,
    mut play_seq: ResMut<crate::creature_anim::PlaySeq>,
) {
    let Some(mut fx) = fx else { return };
    // Resolve the model now, so it loads while the release animation plays.
    for spawn in spawns.read() {
        let key = match (&spawn.path, spawn.ammo_display_id) {
            (Some(path), _) => {
                fx.models
                    .entry(path.clone())
                    .or_insert_with(|| DisplayModel {
                        handle: ModelHandle::M2(asset_server.load(m2_url(path))),
                        ..super::empty_shell()
                    });
                Some(path.clone())
            }
            (None, Some(display_id)) => displays
                .as_deref()
                .and_then(|d| ensure_ammo_model(&mut fx, d, display_id, &asset_server)),
            (None, None) => None,
        };
        let go = QueuedGo {
            spawn: spawn.clone(),
            key,
            queued: 0.0,
            saw_oneshot: false,
        };
        if spawn.awaits_release {
            pending.0.entry(spawn.caster).or_default().push(go);
        } else if let Some(launch) = launch_world_pos(spawn.caster, None, &units, &joints) {
            launch_go(
                &go,
                launch,
                &mut commands,
                &units,
                &joints,
                &mut sounds,
                &mut out,
                &mut play_seq,
            );
        } // else the caster is already gone
    }
    // A release ident fired on a caster launches its whole queue from the event's own position.
    for ev in anim_events.read() {
        if !RELEASE_IDENTS.contains(&ev.ident) {
            continue;
        }
        let Some(gos) = pending.0.remove(&ev.entity) else {
            continue;
        };
        let Some(launch) = launch_world_pos(ev.entity, Some(ev.ident), &units, &joints) else {
            continue;
        };
        for go in &gos {
            launch_go(
                go,
                launch,
                &mut commands,
                &units,
                &joints,
                &mut sounds,
                &mut out,
                &mut play_seq,
            );
        }
    }
    // Age every queue, and flush one whose one-shot ended without its keyframe (the reference's
    // anim-finish `0x5fc920` → `0x60c9b0`) or never started.
    let dt = time.delta_secs();
    let catalog = anim_data.as_deref().map(|d| &d.0);
    let mut flushes: Vec<(Entity, Vec<QueuedGo>)> = Vec::new();
    pending.0.retain(|&caster, gos| {
        if !units.contains(caster) {
            return false; // caster streamed out: its queue dies with it
        }
        let live = casters
            .get(caster)
            .is_ok_and(|(drv, player, tr, anims)| oneshot_is_live(drv, player, tr, anims, catalog));
        let mut flush = false;
        for go in gos.iter_mut() {
            go.queued += dt;
            go.saw_oneshot |= live;
            flush |= (go.saw_oneshot && !live) || (!go.saw_oneshot && go.queued > RELEASE_WAIT_MAX);
        }
        if flush {
            // Launched below: the launch's borrows cannot live inside this closure.
            flushes.push((caster, std::mem::take(gos)));
            return false;
        }
        true
    });
    for (caster, gos) in flushes {
        let Some(launch) = launch_world_pos(caster, None, &units, &joints) else {
            continue;
        };
        for go in &gos {
            launch_go(
                go,
                launch,
                &mut commands,
                &units,
                &joints,
                &mut sounds,
                &mut out,
                &mut play_seq,
            );
        }
    }
}

/// Spawn a missile's parts and emitters as its children once its M2 has built, so they ride the
/// mover; an unloadable model flies invisible and still impacts on time.
pub(super) fn attach_missile_models(
    mut commands: Commands,
    mut missiles: Query<(Entity, &mut Missile)>,
    fx: Option<ResMut<SpellFx>>,
    asset_server: Res<AssetServer>,
    time: Res<Time>,
    mut wow_materials: ResMut<Assets<benilla_assets::materials::WowModelMaterial>>,
    mut tint_reg: ResMut<super::spell_fx::FxTintAnims>,
    mut uv_reg: ResMut<benilla_world::doodad_anim::UvAnimMaterials>,
    mut anim_table: ResMut<benilla_world::mat_anim_table::MatAnimTable>,
    ibps: Res<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut palettes: ResMut<benilla_world::rig_palette::RigPalettes>,
) {
    let Some(mut fx) = fx else {
        return;
    };
    for (entity, mut missile) in &mut missiles {
        if missile.parts_spawned {
            continue;
        }
        let Some(key) = missile.path.clone() else {
            continue; // unreachable: the spawn sets `parts_spawned`
        };
        ensure_model(&mut fx, &asset_server, &key);
        let Some(dm) = fx.models.get(&key) else {
            continue; // unreachable: just inserted
        };
        // `false` means still loading; a later pass attaches mid-flight.
        if !attach_effect_visuals(
            &mut commands,
            entity,
            dm,
            time.elapsed_secs(),
            false, // a missile's flat quads are geometry, never ground decals
            // A free world model: its trail stays world-frozen and its pool finishes in place.
            EffectHost::default(),
            // A `CMissile`, not a `CEffect`: no kit stage and no Birth/Hold/Decay, one sequence.
            None,
            &mut FxMaterials {
                store: &mut wow_materials,
                tint: &mut tint_reg,
                uv: &mut uv_reg,
                table: &mut anim_table,
            },
            &ibps,
            &mut palettes,
            Some(INFLIGHT_ANIM),
        ) {
            continue;
        }
        missile.parts_spawned = true;
    }
}

/// The reference's arrive-on-time step `0x61e2a0`: each frame covers `dt / remaining time` of the
/// gap to the live aim; at the deadline the missile snaps there, hands off and despawns.
pub(super) fn move_missiles(
    mut commands: Commands,
    time: Res<Time>,
    mut missiles: Query<(Entity, &mut Missile, &mut Transform)>,
    units: AttachPosQuery,
    joints: Query<&GlobalTransform>,
    mut out: ArrivalOut,
    mut sounds: MessageWriter<MissileSound>,
    mut play_seq: ResMut<crate::creature_anim::PlaySeq>,
) {
    let dt = time.delta_secs();
    for (entity, mut missile, mut transform) in &mut missiles {
        let Some(aim) = aim_point(missile.aim, &units, &joints) else {
            // A unit target streamed out mid-flight: the reference's `0x61d870` plays on the
            // caster and floats a word per guid from the missile's list (`0x61d9dc`, calling
            // `0x607140`); not built, the flight ends silently.
            sounds.write(MissileSound::Stop { entity });
            commands.entity(entity).despawn();
            continue;
        };
        let to_target = aim - transform.translation;
        if missile.arrive_in <= dt {
            arrival_handoff(&missile, aim, &mut out, &mut play_seq);
            sounds.write(MissileSound::Stop { entity });
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation += to_target * (dt / missile.arrive_in);
        missile.arrive_in -= dt;
        if let Some(dir) = to_target.try_normalize() {
            transform.rotation = missile_facing(dir);
        }
    }
}

/// The pending queue, headless; the anim-end flush edge needs a live animation rig and is not
/// covered here.
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use bevy::asset::AssetPlugin;

    use super::*;
    use crate::creature_anim::PlaySeq;

    /// `Spell.dbc` Speed such that the test target's 24-unit range is a 1.000 s flight.
    const SPEED: f32 = 24.0;

    fn app() -> App {
        let mut app = App::new();
        // No TimePlugin: the tests advance `Time` by hand so every queued-duration is exact.
        app.add_plugins((TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_resource::<Time>();
        app.init_resource::<super::super::spell_fx::SpellFx>();
        app.init_resource::<PendingMissiles>();
        app.init_resource::<PlaySeq>();
        app.add_message::<MissileSpawn>()
            .add_message::<AnimSoundEvent>()
            .add_message::<MissileSound>()
            .add_message::<CastEvent>()
            .add_message::<DefenseAnim>()
            .add_message::<MissileMiss>();
        app.add_systems(Update, spawn_missiles);
        app
    }

    fn step(app: &mut App, dt: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(dt));
        app.update();
    }

    /// A caster whose `$CSL` marker, on bone 0 of its pose, sits at `hand`.
    fn caster(app: &mut App, hand: Vec3) -> Entity {
        let caster = app
            .world_mut()
            .spawn((
                GlobalTransform::default(),
                BoneAttach {
                    points: HashMap::new(),
                    markers: HashMap::from([(*b"$CSL", (0u16, Vec3::ZERO))]),
                },
            ))
            .id();
        let pose = benilla_world::testing::test_rig_pose(caster, &[hand]);
        app.world_mut().entity_mut(caster).insert(pose);
        caster
    }

    fn go(caster: Entity, target: Entity, awaits_release: bool) -> MissileSpawn {
        MissileSpawn {
            caster,
            spell_id: 133,
            path: None, // an invisible flight; the queue needs no model
            ammo_display_id: None,
            dest_tag: None,
            speed: SPEED,
            targets: vec![(target, None)],
            ground_aim: None,
            weapon_visual: None,
            missile_sound: None,
            awaits_release,
        }
    }

    /// The location-fallback flavour of [`go`]: no unit targets, one point on the wire.
    fn ground_go(caster: Entity, at: Vec3) -> MissileSpawn {
        MissileSpawn {
            targets: Vec::new(),
            ground_aim: Some(at),
            ..go(caster, caster, false)
        }
    }

    fn missiles(app: &mut App) -> Vec<(f32, Vec3)> {
        app.world_mut()
            .query::<(&Missile, &Transform)>()
            .iter(app.world())
            .map(|(m, t)| (m.arrive_in, t.translation))
            .collect()
    }

    #[test]
    fn release_keyframe_launches_from_the_marker_minus_queued_time() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 1.5, 0.0))
            .id();
        app.world_mut().write_message(go(caster, target, true));
        step(&mut app, 0.05);
        step(&mut app, 0.05);
        assert!(missiles(&mut app).is_empty(), "parked until the keyframe");
        app.world_mut().write_message(AnimSoundEvent {
            entity: caster,
            ident: *b"$CSL",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        step(&mut app, 0.05);
        let launched = missiles(&mut app);
        assert_eq!(launched.len(), 1);
        let (arrive_in, at) = launched[0];
        assert_eq!(at, hand, "launched from the fired marker's position");
        // 24 units at speed 24 = 1.0 s, minus the 0.10 s spent queued (the two parked frames;
        // the release frame's drain runs before its backstop aging).
        assert!(
            (arrive_in - 0.90).abs() < 1e-3,
            "flight time {arrive_in} ≠ 1.0 − 0.10 queued"
        );
    }

    #[test]
    fn release_past_deadline_impacts_on_the_spot() {
        let mut app = app();
        let caster = caster(&mut app, Vec3::ZERO);
        // 2.4 units at speed 24 = a 0.1 s flight; the queue waits 0.2 s before the keyframe.
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(2.4, 0.0, 0.0))
            .id();
        app.world_mut().write_message(go(caster, target, true));
        step(&mut app, 0.10);
        step(&mut app, 0.10);
        app.world_mut().write_message(AnimSoundEvent {
            entity: caster,
            ident: *b"$CSL",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        step(&mut app, 0.016);
        assert!(missiles(&mut app).is_empty(), "no flight entity");
        let impacts: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<CastEvent>>()
            .drain()
            .filter(|e| matches!(e.kind, CastEventKind::Impact { .. }))
            .collect();
        assert_eq!(impacts.len(), 1, "the impact plays at release");
        assert_eq!(impacts[0].entity, target);
        assert_eq!(impacts[0].spell_id, 133);
    }

    /// The location fallback (`0x6e8a50`'s empty-hit-array arm) flies exactly one projectile.
    #[test]
    fn a_targetless_dest_go_flies_one_projectile_at_the_point() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        let at = Vec3::new(24.0, 1.5, 0.0);
        app.world_mut().write_message(ground_go(caster, at));
        step(&mut app, 0.016);
        let launched = missiles(&mut app);
        assert_eq!(launched.len(), 1, "one shot, not one per (absent) target");
        assert_eq!(launched[0].1, hand, "launched from the caster's marker");
        // 24 units at speed 24, no cast-kit anim to wait on (`awaits_release: false`).
        assert!(
            (launched[0].0 - 1.0).abs() < 1e-3,
            "flight {}",
            launched[0].0
        );
        let ground = app
            .world_mut()
            .query::<&Missile>()
            .iter(app.world())
            .filter(|m| matches!(m.aim, Aim::Ground(p) if p == at))
            .count();
        assert_eq!(ground, 1, "aimed at the point, homing to nothing");
    }

    /// The reference reads the location latch only for an empty hit array (`0x6e8abc`).
    #[test]
    fn a_dest_go_that_hit_units_flies_at_the_units_not_the_point() {
        let mut app = app();
        let caster = caster(&mut app, Vec3::ZERO);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 0.0, 0.0))
            .id();
        let mut spawn = go(caster, target, false);
        spawn.ground_aim = Some(Vec3::new(-50.0, 0.0, 0.0));
        app.world_mut().write_message(spawn);
        step(&mut app, 0.016);
        let aims: Vec<bool> = app
            .world_mut()
            .query::<&Missile>()
            .iter(app.world())
            .map(|m| matches!(m.aim, Aim::Unit { .. }))
            .collect();
        assert_eq!(aims, vec![true], "one unit-homing shot, no ground shot");
    }

    /// `0x61d870` plays the kit on the caster with `extra` = the landing point.
    #[test]
    fn a_ground_arrival_hands_off_to_the_caster_with_the_landing_point() {
        let mut app = app();
        let caster = caster(&mut app, Vec3::ZERO);
        // 2.4 units at speed 24 = a 0.1 s flight, parked 0.2 s → it arrives at the release.
        let at = Vec3::new(2.4, 0.0, 0.0);
        let mut spawn = ground_go(caster, at);
        spawn.awaits_release = true;
        app.world_mut().write_message(spawn);
        step(&mut app, 0.10);
        step(&mut app, 0.10);
        app.world_mut().write_message(AnimSoundEvent {
            entity: caster,
            ident: *b"$CSL",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        step(&mut app, 0.016);
        assert!(missiles(&mut app).is_empty(), "no flight entity");
        let events: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<CastEvent>>()
            .drain()
            .collect();
        assert_eq!(events.len(), 1, "exactly one arrival edge");
        assert_eq!(events[0].entity, caster, "the kit plays on the caster");
        assert_eq!(
            events[0].kind,
            CastEventKind::GroundImpact { pos: at },
            "the ground arm, carrying the landing point"
        );
    }

    #[test]
    fn never_played_oneshot_flushes_after_the_wait_window() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 1.5, 0.0))
            .id();
        app.world_mut().write_message(go(caster, target, true));
        step(&mut app, 0.10);
        step(&mut app, 0.10);
        assert!(missiles(&mut app).is_empty(), "still inside the window");
        step(&mut app, 0.10); // cumulative 0.30 > RELEASE_WAIT_MAX
        let launched = missiles(&mut app);
        assert_eq!(launched.len(), 1, "flushed by the backstop");
        assert_eq!(launched[0].1, hand, "through the $CSL cascade");
        assert!(
            (launched[0].0 - 0.70).abs() < 1e-3,
            "0.30 s queued subtracted"
        );
    }

    /// The `0x61dd50` arm: a miss plays the victim's dodge or block clip, never the impact, and
    /// floats its word over the target. A parry reads Deflect, rewritten at launch; a Reflect
    /// floats over the target, where the inline site re-anchors it to the caster (`0x6e7e51`).
    #[test]
    fn missed_arrival_words_the_target_deflects_a_parry_and_plays_dodge_or_block() {
        for (code, expect_state, expect_word) in [
            (3u8, Some(2u32), "Dodge"),
            (5, Some(5), "Block"),
            (1, None, "Miss"),
            (4, None, "Deflect"),
            (11, None, "Reflect"),
        ] {
            let mut app = app();
            let caster = caster(&mut app, Vec3::ZERO);
            // 2.4 units at speed 24 = a 0.1 s flight; parked 0.2 s → arrival on the spot.
            let target = app
                .world_mut()
                .spawn(GlobalTransform::from_xyz(2.4, 0.0, 0.0))
                .id();
            let mut spawn = go(caster, target, true);
            spawn.targets = vec![(target, Some(code))];
            app.world_mut().write_message(spawn);
            step(&mut app, 0.10);
            step(&mut app, 0.10);
            app.world_mut().write_message(AnimSoundEvent {
                entity: caster,
                ident: *b"$CSL",
                data: 0,
                anim_id: 0,
                pos: None,
            });
            step(&mut app, 0.016);
            assert!(
                missiles(&mut app).is_empty(),
                "no flight entity (code {code})"
            );
            let impacts = app
                .world_mut()
                .resource_mut::<Messages<CastEvent>>()
                .drain()
                .filter(|e| matches!(e.kind, CastEventKind::Impact { .. }))
                .count();
            assert_eq!(impacts, 0, "a miss never plays the impact (code {code})");
            let defenses: Vec<_> = app
                .world_mut()
                .resource_mut::<Messages<DefenseAnim>>()
                .drain()
                .collect();
            match expect_state {
                Some(state) => {
                    assert_eq!(defenses.len(), 1, "one defense clip (code {code})");
                    assert_eq!(defenses[0].victim, target);
                    assert_eq!(defenses[0].victim_state, state);
                }
                None => assert!(defenses.is_empty(), "code {code} plays nothing"),
            }
            let words: Vec<_> = app
                .world_mut()
                .resource_mut::<Messages<MissileMiss>>()
                .drain()
                .map(|w| (w.anchor, w.caster, w.code))
                .collect();
            assert_eq!(
                words,
                vec![(target, caster, if code == 4 { 9 } else { code })],
                "one word over the target (code {code})"
            );
            assert_eq!(
                crate::combat_text::miss_word(words[0].2).map(|(w, _)| w),
                Some(expect_word),
                "code {code} words as {expect_word}"
            );
        }
    }

    /// A GO with no cast-kit animation launches at once (`0x6e7a70`), with the full flight time.
    #[test]
    fn immediate_go_launches_at_once() {
        let mut app = app();
        let hand = Vec3::new(0.0, 1.5, 0.0);
        let caster = caster(&mut app, hand);
        let target = app
            .world_mut()
            .spawn(GlobalTransform::from_xyz(24.0, 1.5, 0.0))
            .id();
        app.world_mut().write_message(go(caster, target, false));
        step(&mut app, 0.016);
        let launched = missiles(&mut app);
        assert_eq!(launched.len(), 1);
        assert_eq!(launched[0].1, hand);
        assert!((launched[0].0 - 1.0).abs() < 1e-3, "full flight time");
    }
}
