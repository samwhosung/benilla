//! The spell-visual data (`SpellVisual.dbc` × `SpellVisualKit.dbc`) and the cast-edge router, the
//! one place spell ids resolve to animations, sounds and effect models.

#[cfg(test)]
mod tests;

use std::cell::Cell;

use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::prelude::*;

use benilla_formats::{SpellVisualCatalog, VisualKit, VisualStages, MISSILE_ATTACH_TABLE};
use benilla_protocol::EntityKind;

use crate::aura_visual::AuraProc;
use crate::entities::ItemDisplays;
use crate::items::{ItemObject, Items};
use crate::net::{FieldChanged, NetCommands, NetEntity, ObjectStore};
use benilla_assets::{LockRecover, WorldAssets};

use super::{
    BaseAnimRecompute, CastEvent, CastEventKind, CastHold, EmoteAnim, SpellGoTargets, WoundAnim,
};

/// The 1.12 client's missile model for an unresolvable visual (`0x860c9c`), shipped in the MPQs.
const ERROR_CUBE: &str = "Spells\\ErrorCube.mdx";

/// The [`SpellKitFx`] reap key for the loot sparkle, outside the spell id range; the reference keys
/// these nodes by attach tag and node flag (`0x61fa10`). A unit wears at most one.
const LOOT_FX_KEY: u32 = u32::MAX;

/// The attachment the loot sparkle, ding and mount poof hang from, else the unit base
/// (`0x61fb4f`). A per-effect table value (`0x80c968`), not a constant: a new user reads its row.
const HARDCODED_FX_ATTACH: u16 = 0x13;

/// The level-up effect's `SpellVisualEffectName` name (`0x8618e0`).
const LEVEL_UP_EFFECT: &str = "HARDCODED Unit Level Up";

/// The mount cloud, hardcoded index 6: row 1185, the druid-morph puff (`0x5ffa50` → `0x61fae0`).
const MOUNT_POOF_EFFECT: &str = "HARDCODED Mount Poof";
/// The meeting-stone join, hardcoded index 0xc (`0x861808`), spawned on the local player by the
/// `SMSG 0x295` status-1 arm (`0x4ca2c7`).
const MEETING_STONE_JOIN_EFFECT: &str = "HARDCODED Meeting Stone Join";

/// The meeting-stone join visual on `entity`: a one-shot at `0x13` (`0x61fae0`, `0x5fbf50`).
pub(crate) fn meeting_stone_join_fx(visuals: &SpellVisuals, entity: Entity) -> Option<SpellKitFx> {
    let (effect, path) = visuals.0.hardcoded_effect(MEETING_STONE_JOIN_EFFECT)?;
    Some(SpellKitFx::Begin {
        entity,
        spell_id: 0,
        persistent: false,
        class: FxClass::Hold,
        stage: FxStage::OneShot,
        effects: vec![FxSlot {
            tag: HARDCODED_FX_ATTACH,
            effect,
            path: path.to_string(),
        }],
    })
}

/// `SpellVisual.dbc` × `SpellVisualKit.dbc`; absent without client data.
#[derive(Resource)]
pub(crate) struct SpellVisuals(pub(crate) SpellVisualCatalog);

/// A kit's sound edge (kit field 13 → `SoundEntries.dbc`), rung at kit start, consumed by
/// `crate::sound` in emission order.
#[derive(Message, Clone, Copy, Debug)]
pub(crate) enum SpellKitSound {
    /// Ring this kit at the unit; a looping kit (`SoundEntries` flag 0x200) becomes the unit's
    /// tracked hold loop until [`Self::StopHold`] (`0x458830` → `0x61fec0`, one-shot `0x458870`).
    Play { entity: Entity, kit_sound: u32 },
    /// The cast or channel hold ended: stop the unit's hold loop (`0x614150`).
    StopHold { entity: Entity },
    /// Ring this kit at a world point, the kit play's `extra` override (`0x60f49e`): a missile's
    /// ground arrival. Always a one-shot: no shipped ground-arrival kit loops.
    PlayAt { pos: Vec3, kit_sound: u32 },
    /// Stop this kit's sound on the unit as its aura leaves (`0x614150`, 0.15 s fade).
    StopKit { entity: Entity, kit_sound: u32 },
}

/// A kit's camera shake (kit field 14 → `SpellEffectCameraShakes.dbc`), consumed by
/// [`crate::camera_shake`]: once per kit play at every stage (`0x620e11`, or `0x60f4e6` with no
/// node), never cancelled.
#[derive(Message, Clone, Copy, Debug)]
pub(crate) enum SpellKitShake {
    /// At the unit's position.
    Play { entity: Entity, group: u32 },
    /// At a world point, the kit play's `extra` override (`0x60f4c1`): a missile's ground arrival.
    PlayAt { pos: Vec3, group: u32 },
}

/// Which owner's reap can kill a persistent effect. The reference's reap `0x614150(spellId,
/// force)` spares stage-2 nodes (flag `0x1000`) unless `force` is set, which only the aura-remove
/// path does (`0x612320` → `0x5ff290`), so a cast's GO never takes the same spell's aura models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FxClass {
    /// Precast and channel holds, reaped by the cast router's edges.
    Hold,
    /// Stage 2 under a live aura, reaped only by [`arm_aura_state_fx`].
    AuraState,
}

/// The kit stage an instance's model runs its animation lifecycle as: `0x60edf0`'s per-stage
/// completion callback (jump table `0x60f4f8`). Independent of [`FxClass`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FxStage {
    /// Stages 0 and 1: destroyed at the first completion (`0x5fbf50`).
    OneShot,
    /// Stage 2: after birth, loop `Hold` (158) if the model has one, `Decay` (159) on reap; a
    /// model with no `Hold` stays parked on its birth (`0x5ff170`).
    State,
    /// Stages 3 and 4: re-arm the sequence that just completed, forever (`0x60ed00`).
    Relive,
}

/// One effect model a play hangs on a unit. Record id and tag are the reference's same-slot
/// replace key: `AddEffect` (`0x61fdd0`) first destroys the owner's nodes with the same record at
/// the same tag (`0x6208e0`), while two rows naming one `.mdx` coexist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FxSlot {
    /// The M2 attachment, or [`benilla_formats::WORLD_EFFECT_TAG`] for the field-12 world plant,
    /// which the replace walk skips (`0x620913`).
    pub(crate) tag: u16,
    /// The `SpellVisualEffectName` record id.
    pub(crate) effect: u32,
    /// The model-cache key.
    pub(crate) path: String,
}

/// A kit's attach-point effect models (kit fields 3 to 11 → `SpellVisualEffectName`, slot loop
/// `0x60f00d`), consumed by `crate::entities::spell_fx` in emission order.
#[derive(Message, Clone)]
pub(crate) enum SpellKitFx {
    /// Attach the kit's models. A persistent one lives until a matching [`Self::Reap`] and
    /// replaces the unit's live persistent instances of the same `(spell_id, class)`; otherwise
    /// it ends after one pass of its first sequence (`0x5fbf50`).
    Begin {
        entity: Entity,
        spell_id: u32,
        persistent: bool,
        class: FxClass,
        /// The models' animation lifecycle ([`FxStage`]).
        stage: FxStage,
        effects: Vec<FxSlot>,
    },
    /// Despawn the unit's persistent effects matching `(spell_id, class)` (`0x614150`).
    Reap {
        entity: Entity,
        spell_id: u32,
        class: FxClass,
    },
}

/// A kit play whose `CharProc` slots name a beam, the dispatcher's chain case (`0x60da79`), run at
/// the kit tail (`0x60f35c`) and from the channel poll (`0x612b18`). Consumed by
/// `crate::entities::chain_beam`.
#[derive(Message, Clone, Copy)]
pub(crate) struct ChainProcPlay {
    /// The caster end, owner of the hop array.
    pub(crate) entity: Entity,
    /// `0` for a bare kit push, which never builds a beam (`0x6ecbd0`).
    pub(crate) spell_id: u32,
    pub(crate) proc: benilla_formats::ChainProc,
}

/// One projectile per target on the GO's Speed > 0 branch (`0x6e8a50` → `0x60a3d0`), consumed
/// by `crate::entities::missile`.
#[derive(Message, Clone)]
pub(crate) struct MissileSpawn {
    pub(crate) caster: Entity,
    pub(crate) spell_id: u32,
    /// `SpellVisual` field 7's model, or `ERROR_CUBE` for an unresolvable id; `None` when field
    /// 7 is below 1 or there is no row, and the spawner flies the wire ammo model (`0x479f40`).
    pub(crate) path: Option<String>,
    /// The GO's ammo display, flown when [`Self::path`] is `None`.
    pub(crate) ammo_display_id: Option<u32>,
    /// The attach tag the missile homes to (`SpellVisual` field 9 through
    /// [`benilla_formats::MISSILE_ATTACH_TABLE`], `0x860a18`); `None` aims at the target's base.
    pub(crate) dest_tag: Option<u16>,
    /// `Spell.dbc` Speed; flight time is distance over speed (`0x61ceb0`).
    pub(crate) speed: f32,
    /// `None` for a hit; for a miss the `SpellMissInfo`, where the missile still flies and arrival
    /// plays the victim's dodge (3) or block (5) clip.
    pub(crate) targets: Vec<(Entity, Option<u8>)>,
    /// With no targets and a DEST point on the GO, one projectile flies at the point (`0x6e8a50`'s
    /// empty-hit arm, `0x6e8aa2`), arriving as [`super::CastEventKind::GroundImpact`].
    /// The reference latches on `flags & 0x60` and aims a SOURCE-only cast at a zeroed DEST
    /// point; this launches only with a real DEST (`flags & 0x40`), a zeroed aim being a quirk
    /// not aped.
    pub(crate) ground_aim: Option<Vec3>,
    /// The caster's ranged fallback visual, carried to arrival when the caster may be gone.
    pub(crate) weapon_visual: Option<u32>,
    /// `SpellVisual` field 10: the sound the projectile loops in flight (`CMissile+0x44`).
    pub(crate) missile_sound: Option<u32>,
    /// Launch at the cast animation's release event (`0x5ffbd0` → `0x60c940`), not at GO; the
    /// reference launches at GO only when the cast kit plays no body animation (`0x6e7a70`).
    pub(crate) awaits_release: bool,
}

/// Load `SpellVisual.dbc` and `SpellVisualKit.dbc` off the patch chain at startup.
pub(super) fn load_spell_visuals(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_spell_visual_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!(
                "anim: {} SpellVisual / {} SpellVisualKit rows",
                cat.len(),
                cat.kit_len()
            );
            commands.insert_resource(SpellVisuals(cat));
        }
        Err(e) => warn!("anim: SpellVisual/SpellVisualKit failed to load: {e:#}"),
    }
}

/// The lookups behind the ranged fallback (`0x60d450`): caster → ranged weapon display →
/// `ItemDisplayInfo` column 10.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct WeaponVisualSrc<'w, 's> {
    displays: Option<Res<'w, ItemDisplays>>,
    items: Option<Res<'w, Items>>,
    net: Option<Res<'w, NetCommands>>,
    // `Without<ItemObject>`: an item's store overlaps the unit indices, so a unit read of it
    // answers item fields.
    units: Query<'w, 's, (Option<&'static NetEntity>, &'static ObjectStore), Without<ItemObject>>,
}

impl WeaponVisualSrc<'_, '_> {
    /// The caster's ranged weapon's substitute `SpellVisual` id.
    fn caster(&mut self, caster: Entity) -> Option<u32> {
        let (net_entity, store) = self.units.get(caster).ok()?;
        let s = &store.0;
        let display_id = match net_entity?.kind {
            EntityKind::Unit => s.unit_virtual_item_display(2),
            // 17: vmangos `EQUIPMENT_SLOT_RANGED`.
            EntityKind::Player => s
                .player_visible_item_entry(17)
                .filter(|e| *e != 0)
                .and_then(|entry| self.items.as_deref()?.held(entry, self.net.as_deref()?))
                .map(|t| t.display_info_id),
            _ => None,
        }
        .filter(|d| *d != 0)?;
        let visual = self
            .displays
            .as_deref()?
            .catalog
            .get(display_id)?
            .spell_visual;
        (visual != 0).then_some(visual)
    }
}

/// A spell's effective `SpellVisual` row (`Spell.dbc` column 115). A ranged spell
/// (`Attributes & 0x2`) fills its row's zero slots, never state or channel, from the caster's
/// ranged weapon visual (`0x60d450`); a basic shot has no row and takes it whole.
fn resolve_stages(
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    spell_id: u32,
    weapon_visual: impl FnOnce() -> Option<u32>,
) -> Option<VisualStages> {
    let def = spells.catalog.get(spell_id)?;
    let own = visuals.stages(def.visual).copied();
    if !def.ranged_slot() {
        return own; // no fill (`0x60d468`)
    }
    let Some(weapon) = weapon_visual().and_then(|v| visuals.stages(v)) else {
        return own;
    };
    // No own row: the reference zeroes the row before the same fill (`0x60d4bc`).
    Some(own.unwrap_or_default().merged_over_weapon(weapon))
}

/// One stage's kit from `spell_id`'s visual chain.
fn resolve_kit(
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    spell_id: u32,
    stage: impl Fn(&VisualStages) -> u32,
    weapon_visual: impl FnOnce() -> Option<u32>,
) -> Option<VisualKit> {
    let kit_id = stage(&resolve_stages(spells, visuals, spell_id, weapon_visual)?);
    (kit_id != 0)
        .then(|| visuals.kit(kit_id).copied())
        .flatten()
}

/// [`resolve_kit`] plus one `fx` trace line saying why a basic shot's kit did or did not resolve:
/// `weapon=not-consulted` (not ranged), `weapon=none` (no ranged visual) or the weapon visual id.
fn resolve_kit_traced(
    stage_name: &str,
    entity: Entity,
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    spell_id: u32,
    stage: impl Fn(&VisualStages) -> u32,
    weapon_visual: impl FnOnce() -> Option<u32>,
) -> Option<VisualKit> {
    // `None`: the fallback never ran (a non-ranged spell); `Some(v)`: it ran and returned `v`.
    let consulted: Cell<Option<Option<u32>>> = Cell::new(None);
    let kit = resolve_kit(spells, visuals, spell_id, &stage, || {
        let visual = weapon_visual();
        consulted.set(Some(visual));
        visual
    });
    if benilla_assets::trace::enabled() {
        let def = spells.catalog.get(spell_id);
        let weapon = match consulted.get() {
            None => "not-consulted".to_string(),
            Some(None) => "none".to_string(),
            Some(Some(visual)) => visual.to_string(),
        };
        // Re-derived from the recorded weapon visual, never a second lookup.
        let kit_id = resolve_stages(spells, visuals, spell_id, || consulted.get().flatten())
            .map_or(0, |s| stage(&s));
        benilla_assets::trace::line(
            "fx",
            &format!(
                "kit {stage_name} unit={entity} spell={spell_id} own_visual={} ranged_slot={} \
                 weapon={weapon} kit={kit_id} anim={:?}",
                def.map_or(0, |d| d.visual),
                def.is_some_and(|d| d.ranged_slot()),
                kit.and_then(|k| k.anim_id),
            ),
        );
    }
    kit
}

/// The in-flight cast's strike sound for the `$TRD` event (`0x62faa0`): `SpellVisual` dword 14
/// (Mining's 1143 "Mining Impact"). No ranged fallback: `$TRD` rides work animations only.
pub(crate) fn held_strike_sound(
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    spell_id: u32,
) -> Option<u32> {
    resolve_stages(spells, visuals, spell_id, || None)?.strike_sound
}

/// A kit's populated effect slots; a slot with no `SpellVisualEffectName` path is skipped.
fn resolve_kit_effects(visuals: &SpellVisualCatalog, kit: &VisualKit) -> Vec<FxSlot> {
    kit.effects()
        .filter_map(|(tag, effect)| {
            visuals.effect_path(effect).map(|path| FxSlot {
                tag,
                effect,
                path: path.to_string(),
            })
        })
        .collect()
}

/// A server-pushed kit (`SMSG_PLAY_SPELL_VISUAL`), played at stage 0 with spell id 0: its effects
/// end only on their own (`0x6e98d0` → `0x60edf0`).
#[derive(Message, Clone, Copy)]
pub(crate) struct KitPush {
    pub(crate) entity: Entity,
    pub(crate) kit_id: u32,
    /// [`super::PlaySeq`] stamp at the wire drain, in packet order.
    pub(crate) seq: u64,
}

/// The kit lanes [`route_cast_visuals`] writes but does not own, bundled into one param because
/// the system is at Bevy's 16-`SystemParam` limit.
type KitSpawnWriters<'w> = (
    MessageWriter<'w, MissileSpawn>,
    MessageWriter<'w, crate::entities::dest_fx::GroundBurst>,
    MessageWriter<'w, ChainProcPlay>,
    MessageWriter<'w, SpellKitShake>,
    MessageWriter<'w, crate::weapon_trail::TrailArm>,
    MessageWriter<'w, BaseAnimRecompute>,
);

/// The writers one kit play fans out to.
struct KitOut<'a, 'w1, 'w2, 'w3, 'w4, 'w5, 'w6, 'w7, 'w8> {
    oneshots: &'a mut MessageWriter<'w1, EmoteAnim>,
    wounds: &'a mut MessageWriter<'w2, WoundAnim>,
    sounds: &'a mut MessageWriter<'w3, SpellKitSound>,
    fx: &'a mut MessageWriter<'w4, SpellKitFx>,
    /// The kit's beam, the dispatcher's chain case.
    chain: &'a mut MessageWriter<'w5, ChainProcPlay>,
    shakes: &'a mut MessageWriter<'w6, SpellKitShake>,
    /// The kit's weapon-trail arm (type-8 case), a latch drawn from the unit's next animation.
    trails: &'a mut MessageWriter<'w7, crate::weapon_trail::TrailArm>,
    /// The base recompute a stage-2 kit's anim id spends itself on.
    recomputes: &'a mut MessageWriter<'w8, BaseAnimRecompute>,
}

/// One kit play's policy on the reference's `PlaySpellVisualKit` (`0x60edf0`):
/// - `persistent`: the effect models outlive the play (a precast or channel hold).
/// - `effects`: whether the models spawn; `false` for the impact hand-off's state stage, whose
///   models [`arm_aura_state_fx`] owns.
/// - `sound`: whether the kit sound rings; its leg has no self test at any stage.
#[derive(Clone, Copy)]
struct KitPlay {
    persistent: bool,
    effects: bool,
    sound: bool,
    /// The models' animation lifecycle ([`FxStage`]).
    stage: FxStage,
    /// The stage-2 (state kit) play (`0x60f383`): `0x60f387` branches around the only anim play
    /// (`0x60f3c5`), so the id is only compared with what the unit plays, a mismatch spent on a
    /// [`BaseAnimRecompute`].
    stage_2: bool,
}

impl KitPlay {
    /// Stages 0 and 1 (`0x5fbf50`): self-terminating models, every leg on.
    const DISCRETE: Self = Self {
        persistent: false,
        effects: true,
        sound: true,
        stage: FxStage::OneShot,
        stage_2: false,
    };
}

fn play_kit(
    entity: Entity,
    spell_id: u32,
    kit: &VisualKit,
    play: KitPlay,
    seq: u64,
    visuals: &SpellVisualCatalog,
    out: &mut KitOut,
) {
    if let Some(anim_id) = kit.anim_id {
        if play.stage_2 {
            // Stage 2 compares, never plays; the driver holds the armed id and compares.
            out.recomputes.write(BaseAnimRecompute { entity, anim_id });
        } else if (8..=10).contains(&anim_id) {
            // A CombatWound id (8 to 10) lays a severity-0 wound in the blend slot, not the
            // kit's own id (`0x60f3ad` → `0x60f510`, `0x60f3b8`).
            out.wounds.write(WoundAnim { entity });
        } else {
            out.oneshots.write(EmoteAnim {
                entity,
                anim_id,
                seq,
            });
        }
    }
    if let Some(kit_sound) = kit.sound.filter(|_| play.sound) {
        out.sounds.write(SpellKitSound::Play { entity, kit_sound });
    }
    // The camera shake plays at every stage, gated on nothing.
    if let Some(group) = kit.shake {
        out.shakes.write(SpellKitShake::Play { entity, group });
    }
    // The beam case runs for every play at every stage (`0x60f35c`), ungated by `play.effects`.
    if let Some(proc) = kit.chain_proc() {
        out.chain.write(ChainProcPlay {
            entity,
            spell_id,
            proc,
        });
    }
    // The trail arm (`0x60d80a`), on the same terms; the reference dispatches it before the kit's
    // anim (`0x60f3c5`), so a kit with both fires its trail on its own swing.
    if let Some(trail) = kit.trail_proc() {
        out.trails
            .write(crate::weapon_trail::TrailArm { entity, trail });
    }
    if !play.effects {
        return;
    }
    let effects = resolve_kit_effects(visuals, kit);
    if !effects.is_empty() {
        out.fx.write(SpellKitFx::Begin {
            entity,
            spell_id,
            persistent: play.persistent,
            class: FxClass::Hold,
            stage: play.stage,
            effects,
        });
    }
}

/// One spell landing on one unit.
#[derive(Clone, Copy)]
struct ImpactPlay {
    spell_id: u32,
    /// The caster's ranged fallback, resolved before the caster can vanish.
    weapon_visual: Option<u32>,
    /// Lay the victim's severity-0 wound flinch beside the kit.
    flinch: bool,
    seq: u64,
}

/// The unit-impact hand-off (`0x61dc50`): the impact kit (stage 1), then the state kit's anim and
/// sound, its models owned by [`arm_aura_state_fx`]. The stage-2 gate `0x61dc20` is not built: its
/// content is untraced. The flinch comes after the kit for a harmful instant hit (`0x6e8bf0`),
/// before it for every missile target.
fn play_impact(
    entity: Entity,
    impact: ImpactPlay,
    spells: &crate::ui_action::Spells,
    visuals: &SpellVisualCatalog,
    out: &mut KitOut,
) {
    let ImpactPlay {
        spell_id,
        weapon_visual,
        flinch,
        seq,
    } = impact;
    if flinch {
        out.wounds.write(WoundAnim { entity });
    }
    for (stage, play) in [
        (
            (|s: &VisualStages| s.impact) as fn(&VisualStages) -> u32,
            KitPlay::DISCRETE,
        ),
        (
            |s: &VisualStages| s.state,
            KitPlay {
                effects: false,
                stage_2: true,
                stage: FxStage::State,
                ..KitPlay::DISCRETE
            },
        ),
    ] {
        if let Some(kit) = resolve_kit(spells, visuals, spell_id, stage, || weapon_visual) {
            play_kit(entity, spell_id, &kit, play, seq, visuals, out);
        }
    }
}

/// The cast-edge router: wire [`CastEvent`]s and the public channel field → animation intents.
///
/// - Start: the precast kit's anim becomes the [`CastHold`] (stage 4) and its sound rings.
/// - Go: the hold drops and the cast kit plays; the GO's targets play their impacts inline when
///   `Spell.dbc` Speed is 0 (`0x6e8bf0`), else fly a [`MissileSpawn`] (`0x6e8a50`).
/// - Impact: a missile arrived on this unit ([`play_impact`]).
/// - Fail: the hold drops silently.
/// - Channel: `UNIT_CHANNEL_SPELL` polled per frame (`0x612a30`); observers have no packet. A set
///   field holds the channel kit's anim and rings its sound once, a cleared one drops it.
pub(super) fn route_cast_visuals(
    mut commands: Commands,
    mut events: MessageReader<CastEvent>,
    mut go_targets: MessageReader<SpellGoTargets>,
    mut pushes: MessageReader<KitPush>,
    mut oneshots: MessageWriter<EmoteAnim>,
    mut wounds: MessageWriter<WoundAnim>,
    mut sounds: MessageWriter<SpellKitSound>,
    mut fx: MessageWriter<SpellKitFx>,
    mut sheaths: MessageWriter<super::SheathRequest>,
    mut spawns: KitSpawnWriters,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut weapon_src: WeaponVisualSrc,
    units: Query<(Entity, &ObjectStore), Without<ItemObject>>,
    holds: Query<&CastHold>,
    mut channel_cache: Local<EntityHashMap<u32>>,
) {
    let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
        return; // no client data
    };
    // Disjoint borrows: the beam writer rides `out` while the missile and burst writers stay free.
    let (missiles, bursts, chains, shakes, trails, recomputes) = (
        &mut spawns.0,
        &mut spawns.1,
        &mut spawns.2,
        &mut spawns.3,
        &mut spawns.4,
        &mut spawns.5,
    );
    let mut out = KitOut {
        oneshots: &mut oneshots,
        wounds: &mut wounds,
        sounds: &mut sounds,
        fx: &mut fx,
        chain: chains,
        shakes,
        trails,
        recomputes,
    };

    // `holds` lags one command flush and an instant cast's START and GO drain together, so
    // `pending` overlays this frame's hold writes (else the cast pose sticks).
    let mut pending: EntityHashMap<Option<u32>> = EntityHashMap::default();
    let held_spell = |pending: &EntityHashMap<Option<u32>>, entity: Entity| -> Option<u32> {
        match pending.get(&entity) {
            Some(&overlaid) => overlaid,
            None => holds.get(entity).ok().map(|h| h.spell_id),
        }
    };

    for p in pushes.read() {
        // No kit-level scan, but each slot's same-(record, tag) replace (`0x6208e0`) swaps the old
        // batch out; the driver's same-id dedup (`0x5fdba0`) keeps a looping clip running.
        if let Some(kit) = visuals.0.kit(p.kit_id) {
            play_kit(
                p.entity,
                0,
                kit,
                KitPlay::DISCRETE,
                p.seq,
                &visuals.0,
                &mut out,
            );
        }
    }

    for ev in events.read() {
        // A despawn applied at the net sync point can outlive a cast edge from the same batch;
        // skip a dead subject whole.
        if commands.get_entity(ev.entity).is_err() {
            continue;
        }
        match ev.kind {
            CastEventKind::Start => {
                // A replacing cast reaps the prior hold's loop and persistent models first.
                out.sounds
                    .write(SpellKitSound::StopHold { entity: ev.entity });
                if let Some(prior) = held_spell(&pending, ev.entity) {
                    out.fx.write(SpellKitFx::Reap {
                        entity: ev.entity,
                        spell_id: prior,
                        class: FxClass::Hold,
                    });
                }
                // A ranged-attribute START draws the ranged weapon (`0x6e78f3`); vmangos sends
                // START once per auto-repeat activation (`Spell.cpp:3448`).
                if spells
                    .catalog
                    .get(ev.spell_id)
                    .is_some_and(|d| d.ranged_attack())
                {
                    sheaths.write(super::SheathRequest {
                        entity: ev.entity,
                        state: 2,
                        ceremony: false,
                    });
                }
                if let Some(kit) = resolve_kit_traced(
                    "precast",
                    ev.entity,
                    spells,
                    &visuals.0,
                    ev.spell_id,
                    |s| s.precast,
                    || weapon_src.caster(ev.entity),
                ) {
                    // The `0x400` weapon-visual hold: a ranged visual play sets it (`0x60d020`),
                    // any other clears it (`0x6ec39e`); it keeps a remote shooter drawn.
                    let ranged = spells
                        .catalog
                        .get(ev.spell_id)
                        .is_some_and(|d| d.ranged_slot());
                    // Fallible: `model_fade::apply_despawn_fade` is unordered against this chain
                    // and can despawn the unit before these commands apply.
                    if ranged {
                        commands.entity(ev.entity).try_insert(super::RangedHold);
                    } else {
                        commands.entity(ev.entity).try_remove::<super::RangedHold>();
                    }
                    if let Some(anim_id) = kit.anim_id {
                        commands.entity(ev.entity).try_insert(CastHold {
                            anim_id,
                            spell_id: ev.spell_id,
                            ranged,
                        });
                        pending.insert(ev.entity, Some(ev.spell_id));
                    }
                    if let Some(kit_sound) = kit.sound {
                        out.sounds.write(SpellKitSound::Play {
                            entity: ev.entity,
                            kit_sound,
                        });
                    }
                    // Persistent until the cast resolves, re-armed forever (stage 4).
                    let effects = resolve_kit_effects(&visuals.0, &kit);
                    if !effects.is_empty() {
                        out.fx.write(SpellKitFx::Begin {
                            entity: ev.entity,
                            spell_id: ev.spell_id,
                            persistent: true,
                            class: FxClass::Hold,
                            stage: FxStage::Relive,
                            effects,
                        });
                    }
                }
            }
            CastEventKind::Go => {
                // Spell-id-keyed (`0x614150(spellId, 0)`): a proc's GO keeps another spell's hold.
                if held_spell(&pending, ev.entity) == Some(ev.spell_id) {
                    commands.entity(ev.entity).try_remove::<CastHold>();
                    pending.insert(ev.entity, None);
                }
                // Reaped whatever the hold: a precast kit with no anim still starts a loop.
                out.sounds
                    .write(SpellKitSound::StopHold { entity: ev.entity });
                out.fx.write(SpellKitFx::Reap {
                    entity: ev.entity,
                    spell_id: ev.spell_id,
                    class: FxClass::Hold,
                });
                // A ranged-slot GO redraws ranged before the cast kit (`0x60f34c`, gate
                // `Attributes & 0x2`, stages 0 and 4): what redraws a bow an emote lowered.
                if spells
                    .catalog
                    .get(ev.spell_id)
                    .is_some_and(|d| d.ranged_slot())
                {
                    sheaths.write(super::SheathRequest {
                        entity: ev.entity,
                        state: 2,
                        ceremony: false,
                    });
                }
                // For a basic shot the cast kit is the fire clip, from the ranged fallback.
                if let Some(kit) = resolve_kit_traced(
                    "cast",
                    ev.entity,
                    spells,
                    &visuals.0,
                    ev.spell_id,
                    |s| s.cast,
                    || weapon_src.caster(ev.entity),
                ) {
                    // The `0x400` hold re-asserted per shot, or cleared for a non-ranged play.
                    if spells
                        .catalog
                        .get(ev.spell_id)
                        .is_some_and(|d| d.ranged_slot())
                    {
                        commands.entity(ev.entity).try_insert(super::RangedHold);
                    } else {
                        commands.entity(ev.entity).try_remove::<super::RangedHold>();
                    }
                    play_kit(
                        ev.entity,
                        ev.spell_id,
                        &kit,
                        KitPlay::DISCRETE,
                        ev.seq,
                        &visuals.0,
                        &mut out,
                    );
                }
            }
            CastEventKind::Impact { weapon_visual } => {
                // The arrival flinches every living target, no hostility test (`0x61dc74`).
                play_impact(
                    ev.entity,
                    ImpactPlay {
                        spell_id: ev.spell_id,
                        weapon_visual,
                        flinch: true,
                        seq: ev.seq,
                    },
                    spells,
                    &visuals.0,
                    &mut out,
                );
            }
            CastEventKind::GroundImpact { pos } => {
                // A missile arrived at a point (`0x61e1d0` → `0x61d870`): `SpellVisual` field 13,
                // the area kit, plays at the landing point. Every shipped row that reaches here
                // carries only a sound, so the anim and slot legs are not built.
                let area = resolve_kit(spells, &visuals.0, ev.spell_id, |s| s.area_kit, || None);
                if let Some(kit_sound) = area.and_then(|k| k.sound) {
                    out.sounds.write(SpellKitSound::PlayAt { pos, kit_sound });
                }
                // Kit 1285 (Cannon Ball) is a shake and nothing else.
                if let Some(group) = area.and_then(|k| k.shake) {
                    out.shakes.write(SpellKitShake::PlayAt { pos, group });
                }
            }
            CastEventKind::Fail => {
                if held_spell(&pending, ev.entity) == Some(ev.spell_id) {
                    commands.entity(ev.entity).try_remove::<CastHold>();
                    pending.insert(ev.entity, None);
                }
                out.sounds
                    .write(SpellKitSound::StopHold { entity: ev.entity });
                out.fx.write(SpellKitFx::Reap {
                    entity: ev.entity,
                    spell_id: ev.spell_id,
                    class: FxClass::Hold,
                });
            }
        }
    }

    // Speed 0 plays every impact now (`0x6e8bf0`), Speed > 0 flies one projectile per target.
    // Misses fly too; the reference deflects them off the target, which is not built.
    for go in go_targets.read() {
        let Some(display) = spells.catalog.get(go.spell_id) else {
            continue;
        };
        // Resolved once per GO (`0x6e802e`); every consumer below reads the merged row.
        let wv = weapon_src.caster(go.caster);
        let stages = resolve_stages(spells, &visuals.0, go.spell_id, || wv);
        // The dest one-shot (`0x6e8088`): field 12's model once at the dest point when field 6 is
        // 0, fired at GO whatever the hit list.
        if let Some(dest) = go.dest {
            if let Some(stages) = stages {
                if stages.missile_gate == 0 && stages.area_effect != 0 {
                    if let Some(path) = visuals.0.effect_path(stages.area_effect) {
                        bursts.write(crate::entities::dest_fx::GroundBurst {
                            path: path.to_string(),
                            pos: dest,
                        });
                    }
                }
            }
        }
        if display.speed <= 0.0 {
            // Each hit flinches after its impact kit iff the spell is harmful (`0x6e8c7b`).
            let flinch = display.is_harmful();
            for &target in &go.hits {
                play_impact(
                    target,
                    ImpactPlay {
                        spell_id: go.spell_id,
                        weapon_visual: wv,
                        flinch,
                        seq: go.seq,
                    },
                    spells,
                    &visuals.0,
                    &mut out,
                );
            }
        } else {
            // The spawn gate is Speed alone (`0x60a3d0`): basic shots have no visual row.
            let path = stages.and_then(|s| {
                (s.missile_model >= 1).then(|| {
                    visuals
                        .0
                        .effect_path(s.missile_model)
                        .unwrap_or(ERROR_CUBE)
                        .to_string()
                })
            });
            // An out-of-table ordinal reads adjacent `.rdata` in the reference; here, the base.
            let dest_tag =
                stages.and_then(|s| MISSILE_ATTACH_TABLE.get(s.missile_attach as usize).copied());
            let missile_sound = stages.and_then(|s| s.missile_sound);
            let targets: Vec<(Entity, Option<u8>)> = go
                .hits
                .iter()
                .map(|&e| (e, None))
                .chain(go.misses.iter().map(|&(e, code)| (e, Some(code))))
                .collect();
            // Only an empty hit array consults the ground point (`0x6e8abc … je 0x6e8ba2`).
            let ground_aim = targets.is_empty().then_some(go.dest).flatten();
            if !targets.is_empty() || ground_aim.is_some() {
                // The release gate (`0x6e7a70`, inverted).
                let awaits_release = stages
                    .filter(|s| s.cast != 0)
                    .and_then(|s| visuals.0.kit(s.cast))
                    .is_some_and(|k| k.anim_id.is_some());
                missiles.write(MissileSpawn {
                    caster: go.caster,
                    spell_id: go.spell_id,
                    path,
                    ammo_display_id: go.ammo_display_id,
                    dest_tag,
                    speed: display.speed,
                    targets,
                    ground_aim,
                    weapon_visual: wv,
                    missile_sound,
                    awaits_release,
                });
            }
        }
    }

    // The channel poll: `UNIT_CHANNEL_SPELL` edges drive an observed unit's channel (the self-only
    // `MSG_CHANNEL_*` do not); a cleared field drops the hold even with no other wire trace.
    for (entity, store) in &units {
        let cur = store.0.unit_channel_spell();
        let prev = channel_cache.get(&entity).copied().unwrap_or(0);
        if cur == prev {
            continue;
        }
        channel_cache.insert(entity, cur);
        if cur != 0 {
            out.sounds.write(SpellKitSound::StopHold { entity });
            if prev != 0 {
                // A channel replacing a channel reaps the old spell's effects first.
                out.fx.write(SpellKitFx::Reap {
                    entity,
                    spell_id: prev,
                    class: FxClass::Hold,
                });
            }
            // No ranged fallback on the channel poll (`0x612a30`).
            if let Some(kit) = resolve_kit(spells, &visuals.0, cur, |s| s.channel, || None) {
                if let Some(anim_id) = kit.anim_id {
                    // Fallible: a unit mid-stream-out is un-indexed but still in this query.
                    commands.entity(entity).try_insert(CastHold {
                        anim_id,
                        spell_id: cur,
                        ranged: false, // no basic shot channels
                    });
                    pending.insert(entity, Some(cur));
                }
                if let Some(kit_sound) = kit.sound {
                    out.sounds.write(SpellKitSound::Play { entity, kit_sound });
                }
                // The CharProc dispatcher's second caller (`0x612b18`): Drain Life's beam is
                // reached only here, on the rising edge, and lives until the field clears.
                if let Some(proc) = kit.chain_proc() {
                    out.chain.write(ChainProcPlay {
                        entity,
                        spell_id: cur,
                        proc,
                    });
                }
                // The trail arm from the same caller: kit 370 (Whirlwind Primer) is reached here.
                if let Some(trail) = kit.trail_proc() {
                    out.trails
                        .write(crate::weapon_trail::TrailArm { entity, trail });
                }
                // Persistent while the field holds, with the stage-2 lifecycle.
                let effects = resolve_kit_effects(&visuals.0, &kit);
                if !effects.is_empty() {
                    out.fx.write(SpellKitFx::Begin {
                        entity,
                        spell_id: cur,
                        persistent: true,
                        class: FxClass::Hold,
                        stage: FxStage::State,
                        effects,
                    });
                }
            }
        } else {
            if held_spell(&pending, entity) == Some(prev) {
                // Only the ending channel's own hold drops; an in-flight precast survives.
                commands.entity(entity).try_remove::<CastHold>();
                pending.insert(entity, None);
            }
            out.sounds.write(SpellKitSound::StopHold { entity });
            out.fx.write(SpellKitFx::Reap {
                entity,
                spell_id: prev,
                class: FxClass::Hold,
            });
        }
    }
    // Streamed units despawn on range-out; drop their cache rows.
    channel_cache.retain(|e, _| units.contains(*e));
}

/// The reference's aura watch span (`0x604d00`, registered at `0x604226`): from
/// `FIELD_UNIT_AURA`, 54 dwords, the 48 slot ids and the 6 packed flag words.
const AURA_WATCH_BASE: u16 = benilla_protocol::field::FIELD_UNIT_AURA;
const AURA_WATCH_SPAN: u16 = 54;

/// Arm and reap each aura's state kit (stage 2) off the unit's `UNIT_AURA` slots: a spell entering
/// the slots begins its kit's models persistent, also on a unit streamed in mid-aura, and leaving
/// reaps them (`0x604d00` → `0x5ff350`; remove `0x612320` → `0x614150(spellId, force=1)`). The add
/// edge rings the kit sound ungated (`0x5ff4c6`) and never plays the kit's anim: stage 2 compares
/// it with what the unit plays, a mismatch spent on a [`BaseAnimRecompute`]. `CharProc` columns go
/// out as [`AuraProc`] edges, and a kit arms for any leg (Stealth's kit 312 is one `CharProc`).
/// `armed` holds only the pairs this watcher began.
pub(crate) fn arm_aura_state_fx(
    // Re-run on an edge inside the aura watch span and on first sight (a standing aura is a
    // state); the full sweep runs once per DBC-resource arrival.
    units: Query<(Entity, &ObjectStore), Without<ItemObject>>,
    arrived: Query<Entity, (Added<ObjectStore>, Without<ItemObject>)>,
    mut edges: MessageReader<FieldChanged>,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut fx: MessageWriter<SpellKitFx>,
    mut procs: MessageWriter<AuraProc>,
    mut sounds: MessageWriter<SpellKitSound>,
    mut recomputes: MessageWriter<BaseAnimRecompute>,
    mut armed: Local<EntityHashMap<Vec<u32>>>,
) {
    let full_sweep = visuals.as_ref().is_some_and(|v| v.is_changed())
        || spells.as_ref().is_some_and(|s| s.is_changed());
    let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
        return; // no client data
    };
    let scan = if full_sweep {
        units.iter().collect::<Vec<_>>()
    } else {
        let mut due: EntityHashSet = arrived.iter().collect();
        due.extend(
            edges
                .read()
                .filter(|e| {
                    e.unit_array_slot(AURA_WATCH_BASE, AURA_WATCH_SPAN)
                        .is_some()
                })
                .map(|e| e.entity),
        );
        due.into_iter().filter_map(|e| units.get(e).ok()).collect()
    };
    for (entity, store) in scan {
        let prev = armed.entry(entity).or_default();
        // Occupied slots, deduped: one state instance per spell however many casters hold it.
        let mut cur: Vec<u32> = store.0.unit_auras().map(|a| a.spell_id).collect();
        cur.sort_unstable();
        cur.dedup();
        for &spell_id in prev.iter() {
            if cur.binary_search(&spell_id).is_err() {
                fx.write(SpellKitFx::Reap {
                    entity,
                    spell_id,
                    class: FxClass::AuraState,
                });
                procs.write(AuraProc::Reap { entity, spell_id });
                // A looping state-kit sound dies with the aura (`0x614150`, 0.15 s fade).
                if let Some(kit_sound) =
                    resolve_kit(spells, &visuals.0, spell_id, |s| s.state, || None)
                        .and_then(|k| k.sound)
                {
                    sounds.write(SpellKitSound::StopKit { entity, kit_sound });
                }
            }
        }
        let mut next = Vec::with_capacity(prev.len());
        for &spell_id in &cur {
            if prev.contains(&spell_id) {
                next.push(spell_id); // still armed
                continue;
            }
            let Some(kit) = resolve_kit(spells, &visuals.0, spell_id, |s| s.state, || None) else {
                continue; // no state kit
            };
            let effects = resolve_kit_effects(&visuals.0, &kit);
            let nodes: Vec<crate::aura_visual::AuraNode> = kit
                .char_procs()
                .filter_map(crate::aura_visual::node_for)
                .collect();
            if effects.is_empty()
                && nodes.is_empty()
                && kit.sound.is_none()
                && kit.anim_id.is_none()
            {
                continue; // nothing to arm or reap
            }
            if !effects.is_empty() {
                fx.write(SpellKitFx::Begin {
                    entity,
                    spell_id,
                    persistent: true,
                    class: FxClass::AuraState,
                    stage: FxStage::State,
                    effects,
                });
            }
            if !nodes.is_empty() {
                procs.write(AuraProc::Begin {
                    entity,
                    spell_id,
                    nodes,
                });
            }
            // Ungated; a same-frame duplicate of the impact hand-off's collapses downstream.
            if let Some(kit_sound) = kit.sound {
                sounds.write(SpellKitSound::Play { entity, kit_sound });
            }
            // The stage-2 anim leg, on the add edge only: an aura refreshed in place keeps its id,
            // and `0x604d00` fires neither arm.
            if let Some(anim_id) = kit.anim_id {
                recomputes.write(BaseAnimRecompute { entity, anim_id });
            }
            next.push(spell_id);
        }
        *prev = next;
    }
    // Streamed units despawn on range-out; drop their rows.
    armed.retain(|e, _| units.contains(*e));
}

/// Arm and reap the loot sparkle on a dead lootable unit and on a corpse object with
/// `CORPSE_FIELD_DYNAMIC_FLAGS` bit 0 (`0x5d6de0`): the `"HARDCODED Loot Art"` row at `0x13`.
/// Edge-driven (`0x600440`) with no viewer or distance logic, since the server strips the flag per
/// viewer; the falling edge reaps with no fade (`0x600680`).
pub(super) fn arm_loot_fx(
    // Re-read on an edge of any of the four fields and on first sight (armed at build, `0x5d6e30`).
    units: Query<(Entity, &ObjectStore, &crate::net::NetEntity)>,
    arrived: Query<Entity, (Added<ObjectStore>, Without<ItemObject>)>,
    mut edges: MessageReader<FieldChanged>,
    visuals: Option<Res<SpellVisuals>>,
    mut fx: MessageWriter<SpellKitFx>,
    mut armed: Local<EntityHashSet>,
    mut tutorials: Option<MessageWriter<crate::tutorial::TutorialEvent>>,
) {
    let full_sweep = visuals.as_ref().is_some_and(|v| v.is_changed());
    let Some((effect, path)) = visuals.as_ref().and_then(|v| v.0.loot_art_effect()) else {
        return; // no client data or no such row
    };
    let scan = if full_sweep {
        units.iter().collect::<Vec<_>>()
    } else {
        use benilla_protocol::field::{
            FIELD_CORPSE_DYNAMIC_FLAGS, FIELD_UNIT_DYNAMIC_FLAGS, FIELD_UNIT_HEALTH,
            FIELD_UNIT_MAXHEALTH,
        };
        let mut due: EntityHashSet = arrived.iter().collect();
        due.extend(
            edges
                .read()
                .filter(|e| {
                    e.unit_field(FIELD_UNIT_DYNAMIC_FLAGS)
                        || e.unit_field(FIELD_UNIT_HEALTH)
                        || e.unit_field(FIELD_UNIT_MAXHEALTH)
                        || (e.kind == benilla_protocol::messages::ObjectType::Corpse
                            && e.index == FIELD_CORPSE_DYNAMIC_FLAGS)
                })
                .map(|e| e.entity),
        );
        due.into_iter().filter_map(|e| units.get(e).ok()).collect()
    };
    for (entity, store, net) in scan {
        // A corpse object's sparkle: bit 0 rising calls `0x5d6e30`, the same row at the same tag.
        let lootable = if net.kind == benilla_protocol::EntityKind::Corpse {
            store.0.corpse_lootable()
        } else {
            store.0.unit_is_dead() && store.0.unit_lootable()
        };
        if lootable && armed.insert(entity) {
            // The Looting tutorial on the rise (`0x60049b`).
            if let Some(t) = tutorials.as_mut() {
                t.write(crate::tutorial::TutorialEvent::trigger(
                    crate::tutorial::id::LOOTING,
                ));
            }
            fx.write(SpellKitFx::Begin {
                entity,
                spell_id: LOOT_FX_KEY,
                persistent: true,
                class: FxClass::Hold,
                // A unit's loot art loops its first sequence (`0x600640`). A corpse's is a bare
                // model with no sequence armed (`0x5d6e30`); this plumbing has no such instance, so
                // it runs `OneShot` persistent.
                stage: if net.kind == benilla_protocol::EntityKind::Corpse {
                    FxStage::OneShot
                } else {
                    FxStage::Relive
                },
                effects: vec![FxSlot {
                    tag: HARDCODED_FX_ATTACH,
                    effect,
                    path: path.to_string(),
                }],
            });
        } else if !lootable && armed.remove(&entity) {
            fx.write(SpellKitFx::Reap {
                entity,
                spell_id: LOOT_FX_KEY,
                class: FxClass::Hold,
            });
        }
    }
    // Streamed units despawn on range-out; drop their rows.
    armed.retain(|e| units.contains(*e));
}

/// The level-up ding: any streamed unit's `UNIT_FIELD_LEVEL` change (`0x6045b0`) spawns
/// [`LEVEL_UP_EFFECT`], independent of `SMSG_LEVELUP_INFO`; a unit streaming in does not ding. Its
/// sound is the model's own `$SND` event.
pub(super) fn arm_level_up_fx(
    mut edges: MessageReader<FieldChanged>,
    visuals: Option<Res<SpellVisuals>>,
    mut fx: MessageWriter<SpellKitFx>,
) {
    for e in edges.read() {
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_LEVEL) {
            continue;
        }
        let Some((effect, path)) = visuals
            .as_ref()
            .and_then(|v| v.0.hardcoded_effect(LEVEL_UP_EFFECT))
        else {
            continue; // no client data or no such row
        };
        debug!(
            "anim: level {} → {}, the ding flashes ({})",
            e.old, e.new, e.entity
        );
        fx.write(SpellKitFx::Begin {
            entity: e.entity,
            spell_id: 0,
            persistent: false,
            class: FxClass::Hold,
            // Self-terminates on its own 1.867 s clip.
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag: HARDCODED_FX_ATTACH,
                effect,
                path: path.to_string(),
            }],
        });
    }
}

/// The mount poof: a `UNIT_FIELD_MOUNTDISPLAYID` change to nonzero on any unit spawns
/// [`MOUNT_POOF_EFFECT`] on the rider as a one-shot (`0x604570` → `0x5ffa50`, gated at `0x5ffa87`).
/// The same-record, same-tag replace (`0x6208e0`) is not built, so a mount swap within 2.8 s would
/// show two clouds.
pub(super) fn arm_mount_poof_fx(
    mut edges: MessageReader<FieldChanged>,
    visuals: Option<Res<SpellVisuals>>,
    mut fx: MessageWriter<SpellKitFx>,
) {
    for e in edges.read() {
        // A dismount spawns nothing; the edge stream skips creation.
        if !e.unit_field(benilla_protocol::field::FIELD_UNIT_MOUNTDISPLAYID) || e.new == 0 {
            continue;
        }
        let Some((effect, path)) = visuals
            .as_ref()
            .and_then(|v| v.0.hardcoded_effect(MOUNT_POOF_EFFECT))
        else {
            continue; // no client data or no such row
        };
        debug!(
            "anim: mount display {} → {}, the poof puffs ({})",
            e.old, e.new, e.entity
        );
        fx.write(SpellKitFx::Begin {
            entity: e.entity,
            spell_id: 0,
            persistent: false,
            class: FxClass::Hold,
            // `0x5fbf50`: destroyed at first completion; the shipped model runs 2.8 s.
            stage: FxStage::OneShot,
            effects: vec![FxSlot {
                tag: HARDCODED_FX_ATTACH,
                effect,
                path: path.to_string(),
            }],
        });
    }
}

/// The pending-morph latch, the reference's `[unit+0xd54]`: armed by `0x5ff0c0` on every aura add
/// and remove of an [`is_morph_spell`] spell; the display rebuild `0x60abe0` replays its impact kit
/// (`0x60ad67`), the shapeshift cloud both ways. Shift-out works because the aura fields apply
/// before `DISPLAYID`.
#[derive(Resource, Default)]
pub(crate) struct MorphLatch(EntityHashMap<u32>);

/// The latch-arm predicate (`0x5ff100`): an effect slot with `Effect` 6 (APPLY_AURA) whose aura is
/// 36 (MOD_SHAPESHIFT) or 56 (TRANSFORM), slot-paired, with no other gate.
fn is_morph_spell(spells: &crate::ui_action::Spells, spell_id: u32) -> bool {
    const SPELL_EFFECT_APPLY_AURA: u32 = 6;
    const SPELL_AURA_MOD_SHAPESHIFT: u32 = 36;
    const SPELL_AURA_TRANSFORM: u32 = 56;
    spells.catalog.get(spell_id).is_some_and(|d| {
        d.effects.iter().zip(&d.effect_apply_aura).any(|(&e, &a)| {
            e == SPELL_EFFECT_APPLY_AURA
                && (a == SPELL_AURA_MOD_SHAPESHIFT || a == SPELL_AURA_TRANSFORM)
        })
    })
}

/// Arm [`MorphLatch`] from aura-slot edges, as the add watcher and the remove tail (`0x6123ad`)
/// both call `0x5ff0c0`: old spell first, new second, so a form swap latches the form entered.
/// First sight emits no edge, as the reference's watchers fire on deltas only.
pub(super) fn arm_morph_latch(
    units: Query<(), With<ObjectStore>>,
    mut edges: MessageReader<FieldChanged>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut latch: ResMut<MorphLatch>,
) {
    if let Some(spells) = spells.as_deref() {
        for e in edges.read() {
            if e.unit_array_slot(
                benilla_protocol::field::FIELD_UNIT_AURA,
                u16::from(benilla_protocol::messages::UNIT_AURA_SLOTS),
            )
            .is_none()
            {
                continue;
            }
            for spell_id in [e.old, e.new] {
                if spell_id != 0 && is_morph_spell(spells, spell_id) {
                    debug!("anim: morph latch armed ({}, spell {spell_id})", e.entity);
                    latch.0.insert(e.entity, spell_id);
                }
            }
        }
    }
    // The latch dies with a despawned unit.
    latch.0.retain(|e, _| units.contains(*e));
}

/// The rebuild's impact-kit replay (the tail of `0x60abe0`): a display swap with the latch armed
/// plays the latched impact kit and clears it, a frame later on the rebuilt body. The same-record,
/// same-tag replace (`0x6208e0`) is not built, so a cold-cache shift-in briefly shows two clouds.
pub(super) fn replay_morph_kit(
    mut swaps: MessageReader<crate::entities::DisplaySwapped>,
    visuals: Option<Res<SpellVisuals>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    mut latch: ResMut<MorphLatch>,
    mut play_seq: ResMut<super::PlaySeq>,
    mut oneshots: MessageWriter<EmoteAnim>,
    mut wounds: MessageWriter<WoundAnim>,
    mut sounds: MessageWriter<SpellKitSound>,
    mut fx: MessageWriter<SpellKitFx>,
    mut chain: MessageWriter<ChainProcPlay>,
    mut shakes: MessageWriter<SpellKitShake>,
    mut trails: MessageWriter<crate::weapon_trail::TrailArm>,
    mut recomputes: MessageWriter<BaseAnimRecompute>,
) {
    for swap in swaps.read() {
        // Cleared even when the kit resolves to nothing (`0x60ad6c`).
        let Some(spell_id) = latch.0.remove(&swap.entity) else {
            continue; // no latch
        };
        let (Some(visuals), Some(spells)) = (visuals.as_deref(), spells.as_deref()) else {
            continue; // no client data
        };
        let Some(kit) = resolve_kit(spells, &visuals.0, spell_id, |s| s.impact, || None) else {
            continue; // no impact kit
        };
        debug!("anim: morph replay ({}, spell {spell_id})", swap.entity);
        let mut out = KitOut {
            oneshots: &mut oneshots,
            wounds: &mut wounds,
            sounds: &mut sounds,
            fx: &mut fx,
            chain: &mut chain,
            shakes: &mut shakes,
            trails: &mut trails,
            recomputes: &mut recomputes,
        };
        play_kit(
            swap.entity,
            spell_id,
            &kit,
            KitPlay::DISCRETE,
            play_seq.next(),
            &visuals.0,
            &mut out,
        );
    }
}
