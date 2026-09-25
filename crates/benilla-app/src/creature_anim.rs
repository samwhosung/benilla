//! Unit animation: a per-unit state machine that picks an `AnimationData.dbc` clip from movement
//! state (the gait by speed and flags, reference selector `0x5fd8b0`; the stand-state and jump
//! Specials; one-shots), cross-fades into it, and plays a locomotion clip at
//! `speed / sequence.moveSpeed` (`0x5fe2f0`). A one-shot plays full-body on a standing idle unit
//! and on the SpineLow torso-masked overlay while moving, seated or airborne in combat
//! (`0x5fe2f0`). Death overrides everything.

use std::time::Instant;

use benilla_assets::{AnimClip, ModelAnimations};
use benilla_formats::AnimDataCatalog;
use bevy::prelude::*;

use benilla_assets::AssetSet;
use benilla_world::schedule::WorldStage;

/// The pure selection logic: the `0x5fd100` tables, the gait, swing and ready picks, the rate math.
pub(crate) mod select;
pub(crate) use select::{
    ease_strafe_yaw, move_flags, strafe_body_offset, swim_body_rotation, MovementState,
};
use select::{Mode, Special};

/// The strafe body counter-twist, composed onto SpineLow and Head after animation.
mod twist;
pub(crate) use twist::{wrap_pi, BodyTwist};

/// Parks an off-frustum rig's pose while its clocks run, as the reference does not tick one
/// (`0x683dd0`); a parked unit's events fire only for a `MORE_AUDIBLE` template.
mod lod;

/// Wielded `(item class, subclass)` per hand as worn (`GetWeapon(slot, 1)`, `0x605e30`, what the
/// paperdoll shows); behaviour reads `armed_main`/`armed_off`, which apply the disarm ladder.
#[derive(Component, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Wielded {
    pub(crate) main: Option<(u8, u8)>,
    pub(crate) off: Option<(u8, u8)>,
    /// The ranged-slot item's `(class, subclass)`, the auto-repeat idle's selector (`0x5fd530`).
    pub(crate) ranged: Option<(u8, u8)>,
    /// The hand items' sheath types (0 none, 1 back 2H, 2 back staff, 3 hip, 4 shield); each picks
    /// its arm's draw/stow clip, HipSheath 90 for types 3 and 7, else Sheath 89 (`0x611930`).
    pub(crate) main_sheath: u8,
    pub(crate) off_sheath: u8,
    /// The ranged sheath type: it picks the draw clip (`0x6118a0`), but the stow is always
    /// Sheath 89 (`0x611b60`).
    pub(crate) ranged_sheath: u8,
    /// The ranged `InventoryType` (0 when empty): `0x1a` and `0x19` draw with the right arm,
    /// anything else (`0x0f`, a bow) with the left (`0x611b60` @ `0x611c74`).
    pub(crate) ranged_inv: u32,
    /// Each held slot's `Material` (main, off, ranged), the draw/stow sound's only key; off the
    /// wire, since the weapon class does not determine it.
    pub(crate) materials: [u8; 3],
    /// `UNIT_FIELD_FLAGS & UNIT_FLAG_DISARMED`, replicated for every unit; the reference reads it
    /// inside `GetWeapon` (`[[unit+0x110]+0xa0] & 0x200000`), for creatures as for players.
    pub(crate) disarmed: bool,
}

/// `UNIT_FIELD_FLAGS` bit 21 (vmangos `UnitDefines.h:566`), tested inside `GetWeapon` at
/// `0x5ec2b8` (CGPlayer) and `0x605e58`/`0x605e98` (CGUnit).
pub(crate) const UNIT_FLAG_DISARMED: u32 = 0x0020_0000;

/// `ItemClass` 2, weapon: the disarm ladder's whole test, so a shield is never the hand it hides.
pub(crate) const ITEM_CLASS_WEAPON: u8 = 2;

/// The one held slot the disarm hides: the main hand if it holds a weapon, else the off hand if it
/// does (`GetWeapon(slot, 0)`, `0x5ec2ae`/`0x5ec262`); the ranged slot is never gated
/// (`0x5ec25e`), as vmangos agrees (`Unit.h:991`). Over classes so the action bar's
/// equipped-item test (`0x5ea5d0`) can use it on raw inventory.
pub(crate) fn disarmed_hand(main_class: Option<u8>, off_class: Option<u8>) -> Option<usize> {
    if main_class == Some(ITEM_CLASS_WEAPON) {
        return Some(0);
    }
    if off_class == Some(ITEM_CLASS_WEAPON) {
        return Some(1);
    }
    None
}

impl Wielded {
    /// The held slot this unit's disarm hides now, `None` while the flag is down.
    pub(crate) fn disarmed_hand(&self) -> Option<usize> {
        self.disarmed
            .then(|| disarmed_hand(self.main.map(|(c, _)| c), self.off.map(|(c, _)| c)))
            .flatten()
    }

    /// The main hand as `GetWeapon(0, 0)` sees it, read by the swing (`0x6246a0`), the Ready idle
    /// (`0x5fcdc0`), the parry table, the sheath machine and the swing sound.
    pub(crate) fn armed_main(&self) -> Option<(u8, u8)> {
        self.main.filter(|_| self.disarmed_hand() != Some(0))
    }

    /// The off hand as `GetWeapon(1, 0)` sees it.
    pub(crate) fn armed_off(&self) -> Option<(u8, u8)> {
        self.off.filter(|_| self.disarmed_hand() != Some(1))
    }
}

/// Auto-attacking (`SMSG_ATTACKSTART` to `ATTACKSTOP`), holding the target guid, the reference's
/// `[+0xc48]`: the Ready idle gates on it, not on sheath (`0x5fd360`); `0x6e3480` reads its reach.
#[derive(Component)]
pub(crate) struct Engaged(pub(crate) u64);

/// The local player's auto-repeat is armed: the reference's `[+0xd58] & 0x200`, set only by the
/// local cast-send (`0x6e593b`); with the ranged sheath drawn a standing unit idles on the Load
/// clip (`0x5fd460`). Cleared only through `cancel_auto_repeat_local`, including on
/// `SMSG_CANCEL_AUTO_REPEAT`, which vmangos sends when a volley's target dies
/// (`SpellCaster.cpp:1829`). A remote shooter plays LoadBow once at its `SMSG_SPELL_START`
/// (`CastHold`, `0x6e7901`), then the fire clip per GO; the reference holds HoldBow (109) from
/// the end of the Load until the first GO.
#[derive(Component)]
pub(crate) struct AutoRepeatArmed;

/// The ranged weapon-visual hold, `[+0xd58] & 0x400`, set for any caster whose ranged visual
/// plays (`0x60d020`); cleared by a non-ranged visual (`0x6ec39e`), the local cancel, melee
/// attack-start or a spline move (`0x6020e8`), never by a sheath change or volley end. It gates
/// nothing here: the reference re-arms the Hold at each completion while it holds (`0x5fc3f0`,
/// via `0x7075af`), ours loops the Hold once armed. It is no entry into the drawn idle: read as
/// one, it leaves a shooter aiming after a single Serpent Sting.
#[derive(Component)]
pub(crate) struct RangedHold;

/// The displayed nocked ammo, the reference's `[+0xd28]`/`[+0xd2c]` (`UpdateAmmoDisplay`
/// `0x60ba30`): an `ItemDisplayInfo` model at HandArrow (35), bow only (`0x60bb19`), set per shot
/// from the `SMSG_SPELL_START` ammo tail. Removed by the un-nock `0x60f530`: the local cancel,
/// leaving the ranged sheath (`0x60fc72`), or a weapon change.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NockedAmmo {
    /// Never 0: an id-0 refresh removes the component.
    pub(crate) display_id: u32,
}

/// The nock latch `[+0xd58] & 0x4000`: set at `$BWP` (`0x624b70`, LoadBow ~0.6 s), cleared at
/// `$BWR` (`0x60016c`, AttackBow ~0.067 s) and on un-nock; the arrow model shows only while set.
#[derive(Component)]
pub(crate) struct NockLatch;

/// `$BWP` latches a unit with `NockedAmmo` cached, `$BWR` clears; runs after `fire_anim_events`
/// so a tag lands the frame it is crossed.
pub(crate) fn drive_nock_latch(
    mut events: MessageReader<AnimSoundEvent>,
    nocked: Query<Has<NockedAmmo>>,
    mut commands: Commands,
) {
    for ev in events.read() {
        match &ev.ident {
            b"$BWP" if nocked.get(ev.entity) == Ok(true) => {
                // `try_insert`: the unit may be despawned by the time the commands apply.
                commands.entity(ev.entity).try_insert(NockLatch);
            }
            b"$BWR" => {
                if let Ok(mut e) = commands.get_entity(ev.entity) {
                    e.remove::<NockLatch>();
                }
            }
            _ => {}
        }
    }
}

/// The one local auto-repeat cancel, reference `0x6ea080`: clears the key (`[0xceac30]`), both
/// idle bits and the nock (`0x60f530`) and sends `CMSG_CANCEL_AUTO_REPEAT_SPELL` (`0x6ea0c6`);
/// the sheath stays drawn.
pub(crate) fn cancel_auto_repeat_local(
    entity: Option<Entity>,
    auto_repeat: &mut crate::spell::AutoRepeatActive,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    if auto_repeat.0.is_none() {
        // The reference's `[0xceac30] == 0` early-out: nothing running, nothing sent.
        return;
    }
    auto_repeat.0 = None;
    if let Some(e) = entity {
        commands
            .entity(e)
            .remove::<(AutoRepeatArmed, RangedHold, NockedAmmo, NockLatch)>();
    }
    let _ = net.0.send(crate::net::ClientCommand::CancelAutoRepeat);
}

/// The local StopAttack, reference `0x5ecac0`: when attacking, `CMSG_ATTACKSTOP` (`0x624370`),
/// then `CancelQueuedCast` (`0x6e6f30`) cancels a queued on-next-swing spell with
/// `CMSG_CANCEL_CAST` (`0x6e4940`), which is why Auto Shot (`0x6e5976`) darkens a Raptor Strike
/// ring. `Engaged` stays until the `SMSG_ATTACKSTOP` echo (`0x624e40`), as in the reference.
pub(crate) fn stop_attack_local(
    engaged: bool,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    net: &crate::net::NetCommands,
) {
    if !engaged {
        // The reference's `!IsAttacking(0x60ecb0) && [+0xc50] == 0` early-out (`[+0xc50]`, a
        // locally started attack, is not modelled): nothing sent, a queued strike survives.
        return;
    }
    let _ = net.0.send(crate::net::ClientCommand::AttackStop);
    if let Some(spell_id) = queued_melee.current() {
        let _ = net
            .0
            .send(crate::net::ClientCommand::CancelCast { spell_id });
        queued_melee.clear_if(spell_id);
    }
}

/// The local StartAttack, reference `0x5ecb70`: the swing send (`0x5eccfd`) is skipped while
/// attacking unless a stop is in flight (`[+0xc54]`, which the target switch passes as
/// `stop_in_flight`); the tail, melee sheath snap then auto-repeat cancel (`0x5ecd78`), always
/// runs, so melee and auto-shot are exclusive. Callers own the entry checks: no call from a cast
/// press while engaged (`0x6e51cb`), the attackability walk (`0x606980`) and `[0xb4b3e4]`.
pub(crate) fn start_attack_local(
    entity: Entity,
    target: u64,
    engaged: bool,
    stop_in_flight: bool,
    auto_repeat: &mut crate::spell::AutoRepeatActive,
    sheath: &mut MessageWriter<SheathRequest>,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    // Starting an attack stands you up (`0x5ecc9b`), after the attackability check but before
    // the range logic and the already-attacking skip, so a re-issued or out-of-reach attack
    // stands you too.
    commands.write_message(crate::player::StandStateRequest { state: 0 });
    if !engaged || stop_in_flight {
        let _ = net
            .0
            .send(crate::net::ClientCommand::AttackSwing { guid: target });
    }
    // Idempotent, matching `0x5ecd80`'s unconditional call.
    sheath.write(SheathRequest {
        entity,
        state: 1,
        ceremony: false,
    });
    cancel_auto_repeat_local(Some(entity), auto_repeat, commands, net);
}

/// The Attack toggle, reference `0x6131a0` (the Attack pseudo-spell, `0x6e4c7a`): stop when
/// attacking, else start. Only the start arm cancels auto-repeat, so toggling melee off leaves
/// Auto Shot running. The stop arm's `[0xb4b3e4]` world gate is not modelled.
pub(crate) fn toggle_attack_local(
    entity: Entity,
    target: u64,
    engaged: bool,
    queued_melee: &mut crate::spell::QueuedMeleeSpell,
    auto_repeat: &mut crate::spell::AutoRepeatActive,
    sheath: &mut MessageWriter<SheathRequest>,
    commands: &mut Commands,
    net: &crate::net::NetCommands,
) {
    if engaged {
        stop_attack_local(engaged, queued_melee, net);
    } else {
        start_attack_local(
            entity,
            target,
            engaged,
            false,
            auto_repeat,
            sheath,
            commands,
            net,
        );
    }
}

/// The attack seams' write set as one param, for the selection writers that run the reference's
/// stop, select, re-swing (`SetSelection` `0x493540`), and the loot its teardown closes.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AttackSeam<'w, 's> {
    pub(crate) net: Res<'w, crate::net::NetCommands>,
    pub(crate) queued_melee: ResMut<'w, crate::spell::QueuedMeleeSpell>,
    pub(crate) auto_repeat: ResMut<'w, crate::spell::AutoRepeatActive>,
    pub(crate) sheath: MessageWriter<'w, SheathRequest>,
    pub(crate) ecs: Commands<'w, 's>,
    /// Our own entity.
    pub(crate) me: Query<'w, 's, Entity, With<crate::net::SelfPlayer>>,
    /// The open loot, which the selection teardown closes when it loots the outgoing target.
    pub(crate) loot: ResMut<'w, crate::ui_loot::LootState>,
    /// The loot latch: the teardown's close drops it, and the right-click loot legs arm it.
    pub(crate) loot_latch: ResMut<'w, crate::ui_loot::LootLatch>,
}

impl AttackSeam<'_, '_> {
    /// The selection teardown's loot close (`0x493910`, `0x493959`–`0x493974`): a window looting
    /// the outgoing selection closes as `CloseLoot` does, its release ahead of anything the new
    /// selection sends. The close's dead-unit deselect (`0x48f369`) targets the selection already
    /// being torn down, so none is asked; the reference's nested teardown there also sends a
    /// `CMSG_SET_SELECTION` 0 (`0x493a2a`), which this does not.
    pub(crate) fn close_loot_on(&mut self, outgoing: u64) {
        if self.loot.source() == Some(outgoing) {
            debug!("target: the selection teardown closes the loot on {outgoing:#x}");
            crate::ui_loot::close_interaction(
                &mut self.loot,
                &mut self.loot_latch,
                Some(&self.net),
            );
        }
    }

    /// StopAttack (`0x5ecac0`).
    pub(crate) fn stop(&mut self, engaged: bool) {
        stop_attack_local(engaged, &mut self.queued_melee, &self.net);
    }

    /// StartAttack (`0x5ecb70`); a no-op before our own entity streams in.
    pub(crate) fn start(&mut self, target: u64, engaged: bool, stop_in_flight: bool) {
        let Ok(e) = self.me.single() else { return };
        start_attack_local(
            e,
            target,
            engaged,
            stop_in_flight,
            &mut self.auto_repeat,
            &mut self.sheath,
            &mut self.ecs,
            &self.net,
        );
    }
}

/// Mid-cast (`SMSG_SPELL_START` to GO or failure), inserted only for a nonzero cast time and
/// removed by the same spell id (the reference's reap `0x614150(spellId, 0)` is keyed).
#[derive(Component, Clone, Copy)]
pub(crate) struct Casting {
    pub(crate) spell_id: u32,
    #[allow(dead_code)]
    pub(crate) until: Option<Instant>,
}

/// The reference's PlayAnimation order: handlers run in packet order and the later play wins
/// bone 0 (`0x5fe2f0`). benilla's swing and cast pipelines are separate streams, so each
/// animation-bearing message carries a stamp (the wire in packet order, scene-time emitters when
/// they fire), and the driver resolves a same-frame collision by the highest.
#[derive(Resource, Default)]
pub(crate) struct PlaySeq(u64);

impl PlaySeq {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }
}

/// A cast lifecycle edge on a streamed unit, off the wire.
#[derive(Message, Clone, Copy)]
pub(crate) struct CastEvent {
    pub(crate) entity: Entity,
    pub(crate) spell_id: u32,
    pub(crate) kind: CastEventKind,
    /// `PlaySeq` stamp, carried to the kit's `EmoteAnim`.
    pub(crate) seq: u64,
}

/// Which cast edge fired.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum CastEventKind {
    /// `SMSG_SPELL_START`: the precast kit persists from here.
    Start,
    /// `SMSG_SPELL_GO`: reaps the precast, plays the cast kit.
    Go,
    /// Died without release (`SMSG_SPELL_FAILED_OTHER`, or our failed `SMSG_CAST_RESULT`).
    Fail,
    /// The spell landed on this unit (a missile arrival, `0x61dc50`): impact kit, then state kit.
    /// `weapon_visual` is the caster's ranged-weapon `SpellVisual` (`0x60d450`), carried from the
    /// GO since the caster may despawn.
    Impact { weapon_visual: Option<u32> },
    /// A projectile with no live target at arrival (`0x61d870`, a despawned target too): the kit
    /// plays on the caster (`entity`) at `pos` (`0x61d8e7`), `SpellVisual` field 13, stage 3. Sent
    /// here only for a dest-targeted GO's missile with no hit (`MissileSpawn::ground_aim`).
    GroundImpact { pos: Vec3 },
}

/// The anim held while a cast is in flight (`SpellVisualKit` field 2): full-body when standing
/// (`[CGUnit+0xb4]`), a looping masked overlay when moving; reaped by `spell_id`.
#[derive(Component, Clone, Copy)]
pub(crate) struct CastHold {
    pub(crate) anim_id: u16,
    pub(crate) spell_id: u32,
    /// A ranged wind-up (`Attributes & 0x2`): the reference brackets it with ranged snaps
    /// (`0x60f34c`, `0x6e5930`), so the sheath reconcile's force-stow never survives the frame.
    pub(crate) ranged: bool,
}

/// A hand's fingers close (`HandsClosed`, 15) purely because a weapon occupies its attach point,
/// not on combat or sheath; a shield or empty hand stays open (`0x60b590`, paperdoll `0x5059a0`).
#[derive(Component, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HandGrip {
    /// A weapon on the right-hand attach point (id 1).
    pub(crate) right: bool,
    /// A weapon on the left-hand attach point (id 2), not a shield.
    pub(crate) left: bool,
}

/// The grip overlay's weight over the unmasked base on the fingers, about 8:1 so they read closed.
pub(crate) const HAND_GRIP_WEIGHT: f32 = 8.0;

/// One melee swing (`SMSG_ATTACKERSTATEUPDATE`): one packet, one animation. `hit_info` `0x4` is
/// off-hand, `0x10000` suppresses the animation, `0x20` is an absorb (`0x6243e0`); the net bridge
/// rewrites a full block's `victim_state` first.
#[derive(Message, Clone, Copy)]
pub(crate) struct SwingMessage {
    pub(crate) attacker: Entity,
    /// The swing's target, when streamed.
    pub(crate) victim: Option<Entity>,
    pub(crate) hit_info: u32,
    /// vmangos `VictimState`: 1 hit, 2 dodge, 3 parry, 4 interrupt, 5 block, 6 evade, 7 immune,
    /// 8 deflect.
    pub(crate) victim_state: u32,
    pub(crate) damage: u32,
    /// `0x625e40`'s display verdict: it gates only the floating number (`0x62440d`); animation,
    /// flinch, blood, sounds and timers run regardless.
    pub(crate) displayed: bool,
    /// `PlaySeq` stamp, in packet order.
    pub(crate) seq: u64,
}

/// A one-shot anim emote over the gait; its clip's WeaponFlags (`0x10`) stow a drawn weapon, and
/// the stow persists after it.
#[derive(Message, Clone, Copy)]
pub(crate) struct EmoteAnim {
    pub(crate) entity: Entity,
    pub(crate) anim_id: u16,
    /// `PlaySeq` stamp; a kit anim inherits its `CastEvent`'s.
    pub(crate) seq: u64,
}

/// A state kit's `AnimID`, compared and never played: both field-4 callers (`0x5ff4c6`,
/// `0x61dced`) pass stage 2, which `0x60edf0` diverts at `0x60f387`, and a mismatch with the armed
/// id (`0x5fdb50`) recomputes the base (`0x5fd9e0`). So it only cuts: Charge's
/// Knockdown (121) falls to Stand, since `0x5fd8b0` never yields its Stun (14). An in-place aura
/// refresh emits nothing (`0x604d00`).
#[derive(Message, Clone, Copy)]
pub(crate) struct BaseAnimRecompute {
    pub(crate) entity: Entity,
    /// The right side of `0x60f390`'s compare.
    pub(crate) anim_id: u16,
}

/// Rears a rider's mount: MountSpecial (94) on the mount child while the rider holds Mount (91)
/// (`0x608d50`); dropped when there is no mount child.
#[derive(Message, Clone, Copy)]
pub(crate) struct MountFlourish {
    pub(crate) unit: Entity,
}

/// `MountFlourish` to an `EmoteAnim` on the unit's mount child.
fn flourish_to_anim(
    mut msgs: MessageReader<MountFlourish>,
    mut out: MessageWriter<EmoteAnim>,
    mut play_seq: ResMut<PlaySeq>,
    mounts: Query<&crate::entities::mount::MountChild>,
) {
    for m in msgs.read() {
        let Ok(child) = mounts.get(m.unit) else {
            debug!("flourish: {:?} has no mount child — dropped", m.unit);
            continue; // dismounted before the flourish landed
        };
        debug!(
            "flourish: MountSpecial(94) one-shot on {:?} (mount of {:?})",
            child.0, m.unit
        );
        out.write(EmoteAnim {
            entity: child.0,
            anim_id: select::MOUNT_SPECIAL,
            seq: play_seq.next(),
        });
    }
}

/// A spell-side wound flinch, the reference's `0x60ea70(unit, 0)` from the kit player
/// (`0x60f3ad`), the instant-hit loop (`0x6e8c89`, harmful only) and the missile impact
/// (`0x61dc74`): CombatWound (9) or StandWound (8) by engagement, in the secondary-blend slot.
#[derive(Message, Clone, Copy)]
pub(crate) struct WoundAnim {
    pub(crate) entity: Entity,
}

/// One `SMSG_SPELL_GO`'s target lists: impacts inline for `Spell.dbc` Speed 0, else missiles (the
/// reference's `0x6e8bf0` and `0x6e8a50`).
#[derive(Message, Clone)]
pub(crate) struct SpellGoTargets {
    pub(crate) caster: Entity,
    pub(crate) spell_id: u32,
    /// Units the spell landed on; each gets the impact hand-off.
    pub(crate) hits: Vec<Entity>,
    /// Missed units with `SpellMissInfo`: the arrival plays Dodge (30) or ShieldBlock (24), never
    /// Parry (`0x61ceb0`), and no impact kit; the deflect glance-off flight is not built.
    pub(crate) misses: Vec<(Entity, u8)>,
    /// A dest-targeted cast's ground point, in bevy coords; not read, the reference's
    /// dest-anchored launch visual is untraced.
    pub(crate) dest: Option<Vec3>,
    /// The GO's ammo block (`castFlags & 0x20`): the ammo's `ItemDisplayInfo` id, the missile model
    /// when the visual chain has none (`0x479f40`).
    pub(crate) ammo_display_id: Option<u32>,
    /// `PlaySeq` stamp, the GO's `CastEvent`'s.
    pub(crate) seq: u64,
}

/// The sheath policy: requests, ceremony overlays, the `AnimationData.dbc` table.
mod sheath;
use sheath::{load_anim_data, SheathSwap};
pub(crate) use sheath::{
    toggle_sheath_next, AnimData, SheathRequest, SheathSwapMessage, VisualSheath,
};

/// The `SMSG_EMOTE` to `EmoteAnim` consumer.
mod emote_anim;

/// The client-local gestures (chat, NPC interact), the reference's `0x60bb30`.
mod gesture;
use emote_anim::emote_to_anim;
pub(crate) use gesture::{select_gesture, Gesture, GestureQueue};

/// The Bevy systems that execute the state machine `select` picks.
mod driver;
pub(crate) use driver::oneshot_is_live;
use driver::{drive_animations, drive_hand_grip};

/// The model event-keyframe scanner.
mod events;
use events::fire_anim_events;
pub(crate) use events::{
    advance_track, footfall_culls, footfall_side, is_footstep_sound, scan_events, AnimSoundEvent,
    EventFrame, TrackMemory,
};

/// The `$BTH` breath puffs in a snow zone.
mod breath;
use breath::{classify_breath, fire_breath};

mod impact;
use impact::route_swing_impacts;
pub(crate) use impact::{DefenseAnim, PendingImpacts, SwingFlush, SwingImpact, SwingSlowdown};

/// The melee blood spurt.
mod blood;
mod env_damage;
pub(crate) mod net;
pub(crate) mod spell_visual;
use blood::{blood_spurts, load_blood_tables};
use env_damage::{hard_landing_dust, load_env_damage_table};
pub(crate) use env_damage::{EnvDamageTable, HardLanding, HARD_LANDING_DESCENT};
use spell_visual::{
    arm_aura_state_fx, arm_level_up_fx, arm_loot_fx, arm_morph_latch, arm_mount_poof_fx,
    load_spell_visuals, replay_morph_kit, route_cast_visuals, MorphLatch,
};
// `crate::aura_visual`'s tests drive the aura-slot watcher directly.
#[cfg(test)]
pub(crate) use spell_visual::arm_aura_state_fx as arm_aura_state_fx_for_test;
pub(crate) use spell_visual::{
    held_strike_sound, ChainProcPlay, FxClass, FxStage, KitPush, MissileSpawn, SpellKitFx,
    SpellKitShake, SpellKitSound, SpellVisuals,
};

/// The per-unit animation state machine; `Transform` is required, its scale divides the rate.
#[derive(Component)]
#[require(Transform)]
pub(crate) struct AnimDriver {
    mode: Mode,
    /// The targeted gait id, to cross-fade only on a change; `None` forces a fresh pick.
    gait: Option<u16>,
    /// The movement flags the base was last armed for: the reference has no per-one-shot latch,
    /// so a root landing with a cast one-shot (Ice Block) still lets the base overwrite bone 0.
    gait_flags: u32,
    /// The reference's `[unit+0xd58] & 0xc0000`: while Knockdown, LiftOff or Land holds it, base
    /// requests are refused, so a stun's knockdown survives the root's Stand recompute.
    base_lock: driver::play::BaseAnimLock,
    /// The committed sheath state, the reference's `[unit+0xd40]`: seeded from the descriptor,
    /// re-adopted when it changes (`0x604c70`), overwritten by requests and the reconcile.
    sheath_cur: Option<u8>,
    /// The last descriptor sheath byte, to detect changes.
    sheath_byte: Option<u8>,
    /// The pending mid-animation weapon swap while a draw/stow one-shot plays.
    sheath_swap: Option<SheathSwap>,
    /// Whether any animation started this frame, for the weapon-trail latch: the reference reads
    /// `unit+0xd1c` in `PlayAnimation` (`0x5fe48e`), its single entry point, locomotion
    /// included, so starting to run starts Charge's trail.
    started_anim: bool,
    /// A masked upper-body one-shot on the SpineLow overlay while the base drives the legs; a
    /// full-body one lives in `Mode::Swing` until locomotion transplants it here.
    overlay: Option<Overlay>,
    /// The key-bone cross-fade, the reference's per-bone secondary (λ 1 to 0): 150 ms to rest
    /// after a one-shot (`0x5fc920`, `0x7123af`) or a re-arm over the new clip's blendTime
    /// (`0x7125f2`); a transplant (`blendFlag = 0`) seeds none.
    overlay_fade: Option<OverlayFade>,
    /// The wound flinch, the reference's secondary blend slot (λ 0.75 to 0 over the clip), apart
    /// from `overlay` so a masked swing and a wound coexist.
    wound: Option<Wound>,
    /// Whether this arc launched upward (a jump) or is a step-off fall, from vertical speed on
    /// its first airborne frame; the reference carries it as `MSG_MOVE_JUMP`.
    jump_arc: bool,
    /// Last frame's vertical speed: a jump out of a one-frame detachment never toggles FALLING,
    /// so a rise past `JUMP_ARC_MIN_UP` from below also classifies the arc.
    last_vertical_speed: f32,
    /// FALLING last frame: marks takeoff and a step-off fall's landing (`0x602c60` land pick).
    was_falling: bool,
    /// The reference's `CGUnit+0xd60`: a combat clip requested over another doubles the playing
    /// clip's rate and parks here until no one-shot is live; any normal arm clears it (`0x5fe48e`).
    deferred: Option<u16>,
    /// The looping arm's node and rolled budget `R` (`block+0xbc`, `0x7126d8`): the watchdog
    /// (`0x719370`) re-picks after `R` passes. `None` for a one-shot or a freeze.
    loop_window: Option<(bevy::animation::graph::AnimationNodeIndex, u32)>,
    /// The rate last written to the base slot, recorded for the hover inspector.
    gait_rate: f32,
    /// The last mount display id: any change arms bone 0, like the reference's field watcher
    /// (`0x604329`): Mount 91 (`0x607b44`) or Stand 0 (`0x607ce0`), displacing a one-shot.
    mount_display: u32,
    /// The Special wanted last frame. Its edge is a play in the reference and clears the deferred
    /// cache (`0x5fe48e`); its level must not, so a fast-path park survives mid-air.
    last_special: Option<Special>,
    /// The airborne snapshot node `leave_special` stopped, named so the rate sync does not
    /// restart it (a standing locomotion clip also reads rate 0). A symptom fix for the swim hop's
    /// short kick, its mechanism open: the reference's blend source keeps running, so it stays
    /// scoped to the airborne cut.
    frozen: Option<bevy::animation::graph::AnimationNodeIndex>,
}

impl AnimDriver {
    /// The requested `(base, masked overlay)` ids, for the inspector.
    pub(crate) fn playing(&self) -> (Option<u16>, Option<u16>) {
        let base = match self.mode {
            Mode::Gait => self.gait,
            Mode::Entering(s) => Some(s.enter()),
            Mode::Looping(s) => Some(s.loop_id()),
            Mode::Exiting(_, id) => Some(id),
            Mode::Land { id, .. } => Some(id),
            Mode::Swing { id, .. } => Some(id),
        };
        (base, self.overlay.map(|o| o.id))
    }

    /// The base slot's playback rate, for the inspector.
    pub(crate) fn rate(&self) -> f32 {
        self.gait_rate
    }
}

/// A masked upper-body play: its node and requested id; `looping` marks a cast hold, released
/// only by the driver's hold logic.
#[derive(Clone, Copy)]
struct Overlay {
    node: bevy::animation::graph::AnimationNodeIndex,
    id: u16,
    looping: bool,
}

/// A key-bone cross-fade: `out` holds the outgoing pose (`0x7123af`), `None` over the base;
/// `left`/`total` span 150 ms or the incoming clip's blendTime.
#[derive(Clone, Copy)]
struct OverlayFade {
    out: Option<bevy::animation::graph::AnimationNodeIndex>,
    /// Seconds left; λ = `select::blend_lambda(left / total)`.
    left: f32,
    total: f32,
}

/// A wound-flinch decay: the reference seeds `λ₀ = 0.75`, `rate = 1/span` (`0x712647`) and
/// self-releases (`0x7147b9`).
#[derive(Clone, Copy)]
struct Wound {
    node: bevy::animation::graph::AnimationNodeIndex,
    /// The wound clip's length in seconds.
    span: f32,
    /// Masked to the upper body, where the base and a one-shot overlay also blend.
    masked: bool,
}

impl Default for AnimDriver {
    fn default() -> Self {
        Self {
            mode: Mode::Gait,
            gait: None,
            gait_flags: 0,
            base_lock: driver::play::BaseAnimLock::default(),
            sheath_cur: None,
            sheath_byte: None,
            sheath_swap: None,
            started_anim: false,
            overlay: None,
            overlay_fade: None,
            wound: None,
            jump_arc: false,
            last_vertical_speed: 0.0,
            was_falling: false,
            deferred: None,
            loop_window: None,
            gait_rate: 1.0,
            mount_display: 0,
            last_special: None,
            frozen: None,
        }
    }
}

impl AnimDriver {
    /// The requested id playing now; the mouse pick's bounds follow it (`0x7089c0`).
    pub(crate) fn active_anim(&self) -> Option<u16> {
        match self.mode {
            Mode::Gait => self.gait,
            Mode::Entering(sp) => Some(sp.enter()),
            Mode::Looping(sp) => Some(sp.loop_id()),
            Mode::Exiting(_, exit) => Some(exit),
            Mode::Land { id, .. } => Some(id),
            Mode::Swing { id, .. } => Some(id),
        }
    }

    /// `active_anim` through the model's fallback, the sequence actually playing (what the
    /// reconcile reads, `0x5fdb50`); the mode machine keeps comparing the requested id.
    pub(crate) fn resolved_anim(
        &self,
        anims: &ModelAnimations,
        catalog: Option<&AnimDataCatalog>,
    ) -> Option<u16> {
        self.active_anim().map(|id| resolved_id(anims, id, catalog))
    }

    /// The client-side sheath state (0 stowed, 1 melee, 2 ranged), `None` until driven.
    pub(crate) fn sheath_state(&self) -> Option<u8> {
        self.sheath_cur
    }

    /// Whether this unit started an animation this frame.
    pub(crate) fn started_anim(&self) -> bool {
        self.started_anim
    }

    /// Stands in for the driver's write in a consumer's test.
    #[cfg(test)]
    pub(crate) fn set_started_anim(&mut self, played: bool) {
        self.started_anim = played;
    }

    /// A draw/stow ceremony is in flight: the toggle's debounce (`ToggleSheath` guards 11, 12).
    pub(crate) fn sheath_ceremony_active(&self) -> bool {
        self.sheath_swap.is_some()
    }
}

/// Drives unit animation after `WorldStage::Net`, so it reads the current frame's movement.
pub(crate) struct CreatureAnimPlugin;

impl Plugin for CreatureAnimPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        twist::plugin(app);
        lod::plugin(app);
        breath::register(app);
        // The breath classifier runs at its own 10 s cadence, outside the chain.
        app.add_systems(
            Update,
            classify_breath.after(benilla_world::terrain_stream::AreaAuthoritySet),
        );
        app.add_message::<AnimSoundEvent>()
            .add_message::<SwingMessage>()
            .add_message::<SwingImpact>()
            .add_message::<SwingFlush>()
            .add_message::<DefenseAnim>()
            .add_message::<SwingSlowdown>()
            .init_resource::<PendingImpacts>()
            .init_resource::<PlaySeq>()
            .init_resource::<gesture::GestureQueue>()
            .add_message::<EmoteAnim>()
            .add_message::<BaseAnimRecompute>()
            .add_message::<MountFlourish>()
            .add_message::<WoundAnim>()
            .add_message::<SheathSwapMessage>()
            .add_message::<SheathRequest>()
            .add_message::<CastEvent>()
            .add_message::<SpellGoTargets>()
            .add_message::<SpellKitSound>()
            .add_message::<SpellKitShake>()
            .add_message::<SpellKitFx>()
            .add_message::<MissileSpawn>()
            .add_message::<ChainProcPlay>()
            .add_message::<KitPush>()
            // The aura CharProc edges, drained by `crate::aura_visual`.
            .add_message::<crate::aura_visual::AuraProc>()
            .add_message::<HardLanding>()
            // The pending-morph latch, the reference's per-unit `[+0xd54]`.
            .init_resource::<MorphLatch>()
            .add_systems(
                Startup,
                (
                    load_anim_data,
                    load_spell_visuals,
                    load_blood_tables,
                    load_env_damage_table,
                )
                    .after(AssetSet::Open),
            )
            .add_systems(
                Update,
                (
                    route_cast_visuals,
                    arm_loot_fx,
                    arm_level_up_fx,
                    arm_mount_poof_fx,
                    arm_aura_state_fx,
                    // `0x5ff0c0` arms, `0x60abe0` replays on a display swap; arm first, since the
                    // swap crosses from last frame's teardown, armed by the same wire burst.
                    (arm_morph_latch, replay_morph_kit).chain(),
                    blood_spurts,
                    hard_landing_dust,
                    emote_to_anim,
                    gesture::drive_gestures,
                    // Before the driver, so the flourish lands the frame it arrived.
                    flourish_to_anim,
                    drive_animations,
                    // Right after the driver, which rewrites `AnimDriver::started_anim` each pass.
                    crate::weapon_trail::fire_weapon_trails,
                    drive_hand_grip,
                    fire_anim_events,
                    route_swing_impacts,
                    // Before the entity visuals, so the arrow changes on the keyframe's frame.
                    drive_nock_latch,
                    // The ranged prop's clip off the same keys: the bow bends on `$BWP`, a gun's
                    // BowRelease (161) blast plays on `$BWR`.
                    crate::ranged_flex::flex_ranged_props
                        // The material samplers must see the clip armed this frame.
                        .before(benilla_world::model_render::ModelVisSet),
                    // The un-nock reset (`0x60f59d`), off the same `$BWR`: after the arm, or its
                    // skip-while-releasing guard cancels a gun's blast on its first frame.
                    crate::ranged_flex::reset_ranged_props_on_unnock
                        .before(benilla_world::model_render::ModelVisSet),
                    fire_breath,
                )
                    .chain()
                    .after(WorldStage::Net)
                    // Same frame as the loot kneel: the reference force-plays Loot 50 in the
                    // handler that arms the latch.
                    .after(crate::ui_loot::resolve_loot_kneel)
                    // After Input so a sheath request (the Z toggle) executes the same frame.
                    .after(WorldStage::Input)
                    // `VisualSheath` must land before `resolve_equipment` reads it, or a sheath
                    // change double-swaps the weapon placement for a frame.
                    .before(crate::entities::EntityVisualsSet),
            )
            // The CharProc 11 freeze, after the driver so this frame's arms are in when it holds
            // the clocks: Ice Block's cast one-shot is armed, then never advances.
            .add_systems(
                Update,
                crate::aura_visual::apply_aura_anim_rate
                    .after(drive_animations)
                    .after(crate::aura_visual::drain_aura_procs),
            );
    }
}

/// The id this model plays for `id` (`0x711bf0`); identity until `AnimationData.dbc` loads.
fn resolved_id(anims: &ModelAnimations, id: u16, catalog: Option<&AnimDataCatalog>) -> u16 {
    catalog.map_or(id, |cat| anims.resolve(id, cat).id)
}

/// The clip for a requested id, through the model's baked fallback.
fn find_resolved<'a>(
    anims: &'a ModelAnimations,
    id: u16,
    catalog: Option<&AnimDataCatalog>,
) -> Option<&'a AnimClip> {
    anims.find(resolved_id(anims, id, catalog))
}

#[cfg(test)]
mod attack_stand_tests {
    use super::*;

    /// StartAttack stands a seated player (`0x5ecc9b`) even on the already-engaged skip, which
    /// jumps past the stand to `0x5ecd78` and sends no `CMSG_ATTACKSWING`.
    #[test]
    fn starting_an_attack_stands_you_even_when_the_swing_is_skipped() {
        let mut app = App::new();
        app.add_message::<SheathRequest>()
            .add_message::<crate::player::StandStateRequest>();
        // A dead-letter net channel: no packet is under test.
        let (tx, _rx) = crossbeam_channel::unbounded();
        app.insert_resource(crate::net::NetCommands(tx));
        app.insert_resource(crate::spell::AutoRepeatActive(None));

        let me = app.world_mut().spawn_empty().id();
        app.add_systems(
            Update,
            move |mut commands: Commands,
                  mut auto_repeat: ResMut<crate::spell::AutoRepeatActive>,
                  mut sheath: MessageWriter<SheathRequest>,
                  net: Res<crate::net::NetCommands>| {
                start_attack_local(
                    me,
                    0xdead_beef,
                    true, // already engaged: the skip leg
                    false,
                    &mut auto_repeat,
                    &mut sheath,
                    &mut commands,
                    &net,
                );
            },
        );
        app.update();

        let stands: Vec<u8> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<crate::player::StandStateRequest>>()
            .drain()
            .map(|r| r.state)
            .collect();
        assert_eq!(
            stands,
            vec![0],
            "StartAttack asks for stand state 0 exactly once, even on the leg that sends no swing"
        );
    }
}

#[cfg(test)]
mod nock_latch_tests {
    use super::*;

    /// The `$BWP`/`$BWR` nock cycle (set `0x624b70`, cleared `0x60016c`).
    #[test]
    fn bwp_latches_and_bwr_clears_the_nock() {
        let mut app = App::new();
        app.add_message::<AnimSoundEvent>();
        app.add_systems(Update, drive_nock_latch);
        let unit = app.world_mut().spawn(NockedAmmo { display_id: 5996 }).id();
        app.world_mut().write_message(AnimSoundEvent {
            entity: unit,
            ident: *b"$BWP",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
        assert!(
            app.world().entity(unit).contains::<NockLatch>(),
            "$BWP latches a unit with a cached ammo display"
        );
        app.world_mut().write_message(AnimSoundEvent {
            entity: unit,
            ident: *b"$BWR",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
        assert!(
            !app.world().entity(unit).contains::<NockLatch>(),
            "$BWR clears — the arrow leaves with the shot"
        );
        let bare = app.world_mut().spawn_empty().id();
        app.world_mut().write_message(AnimSoundEvent {
            entity: bare,
            ident: *b"$BWP",
            data: 0,
            anim_id: 0,
            pos: None,
        });
        app.update();
        assert!(
            !app.world().entity(bare).contains::<NockLatch>(),
            "no ammo display, no latch"
        );
    }
}

#[cfg(test)]
mod disarm_tests {
    use super::*;

    /// The ladder hides one weapon, main hand first: a disarmed dual-wielder punches with the
    /// main hand and still swings its off-hand weapon (`0x5ec240`, `0x605e30`).
    #[test]
    fn disarm_hides_one_weapon_main_hand_first() {
        let worn = Wielded {
            main: Some((2, 0x7)),   // 1H sword
            off: Some((2, 0xf)),    // dagger
            ranged: Some((2, 0x2)), // bow
            ..Default::default()
        };
        let disarmed = Wielded {
            disarmed: true,
            ..worn
        };

        assert_eq!(worn.disarmed_hand(), None);
        assert_eq!(select::swing_anim_main(worn.armed_main()), 17); // Attack1H
        assert_eq!(select::swing_anim_off(worn.armed_off()), 88); // AttackOffPierce
        assert_eq!(select::ready_anim(worn.armed_main()), 26); // Ready1H

        // The main hand's claim skips the off-hand probe (`0x5ec28d je 0x5ec2aa`).
        assert_eq!(disarmed.disarmed_hand(), Some(0));
        assert_eq!(disarmed.armed_main(), None);
        assert_eq!(disarmed.armed_off(), Some((2, 0xf)), "the off hand is KEPT");
        assert_eq!(select::swing_anim_main(disarmed.armed_main()), 16); // AttackUnarmed
        assert_eq!(select::swing_anim_off(disarmed.armed_off()), 88); // still the dagger
        assert_eq!(select::ready_anim(disarmed.armed_main()), 25); // ReadyUnarmed

        // The ranged slot has no gate (`0x5ec25e`).
        assert_eq!(disarmed.ranged, Some((2, 0x2)));
        assert_eq!(select::ranged_load_anim(disarmed.ranged), 105); // LoadBow

        assert_eq!(disarmed.main, worn.main);
    }

    /// Only a weaponless main hand passes the disarm to the off hand: AttackUnarmedOff (117,
    /// `AnimationData.dbc` fallback 87).
    #[test]
    fn an_empty_main_hand_hands_the_disarm_to_the_off_hand() {
        let off_only = Wielded {
            main: None,
            off: Some((2, 0xf)),
            disarmed: true,
            ..Default::default()
        };
        assert_eq!(off_only.disarmed_hand(), Some(1));
        assert_eq!(off_only.armed_off(), None);
        assert_eq!(select::swing_anim_off(off_only.armed_off()), 117);

        let torch = Wielded {
            main: Some((15, 0)), // class 15 = miscellaneous
            off: Some((2, 0xf)),
            disarmed: true,
            ..Default::default()
        };
        assert_eq!(torch.disarmed_hand(), Some(1));
        assert_eq!(
            torch.armed_main(),
            Some((15, 0)),
            "a non-weapon is not taken"
        );
        assert_eq!(torch.armed_off(), None);
    }

    /// Neither hand holds a weapon: the flag hides nothing.
    #[test]
    fn the_ladder_takes_weapons_and_leaves_the_rest() {
        let shielded = Wielded {
            main: None,
            off: Some((4, 6)), // class 4 = ARMOR, subclass 6 = shield
            disarmed: true,
            ..Default::default()
        };
        assert_eq!(shielded.disarmed_hand(), None);
        assert_eq!(shielded.armed_off(), Some((4, 6)));
    }

    /// The pure ladder over classes, as the action bar's equipped-item test reads it.
    #[test]
    fn the_ladder_over_classes() {
        assert_eq!(disarmed_hand(Some(2), Some(2)), Some(0));
        assert_eq!(disarmed_hand(Some(2), None), Some(0));
        assert_eq!(disarmed_hand(None, Some(2)), Some(1));
        assert_eq!(disarmed_hand(Some(4), Some(2)), Some(1));
        assert_eq!(disarmed_hand(None, None), None);
        assert_eq!(disarmed_hand(Some(4), Some(4)), None);
    }
}
