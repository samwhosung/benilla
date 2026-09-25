//! The death packet handlers filling [`DeathNet`], the movement-mode forward for our own mover,
//! and the corpse-guid latch's hooks on the object lifecycle, called from the drain's object arms.

use benilla_protocol::{EntityKind, MoveMode, ObjectFields, SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{CorpsePoint, DeathNet, ResurrectOffer};
use crate::net::{MoveModeMessage, NetHandlerApp, SelfGuid};

/// Register the death handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::CorpseQuery, on_corpse_query)
        .net_handler(K::CorpseReclaimDelay, on_corpse_reclaim_delay)
        .net_handler(K::ResurrectRequest, on_resurrect_request)
        .net_handler(K::SpiritHealerConfirm, on_spirit_healer_confirm)
        .net_handler(K::DurabilityDamageDeath, on_durability_damage_death)
        .net_handler(K::MoveMode, on_move_mode)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_corpse_query(In(ev): In<SessionEvent>, mut death_net: ResMut<DeathNet>) {
    if let SessionEvent::CorpseQuery {
        found,
        display_map,
        position,
        corpse_map,
    } = ev
    {
        corpse_query(found, display_map, position, corpse_map, &mut death_net);
    }
}

fn on_corpse_reclaim_delay(
    In(ev): In<SessionEvent>,
    mut death_net: ResMut<DeathNet>,
    real_clock: Res<Time<Real>>,
) {
    if let SessionEvent::CorpseReclaimDelay { delay_ms } = ev {
        corpse_reclaim_delay(delay_ms, real_clock.elapsed_secs_f64(), &mut death_net);
    }
}

fn on_resurrect_request(In(ev): In<SessionEvent>, mut death_net: ResMut<DeathNet>) {
    if let SessionEvent::ResurrectRequest {
        caster,
        name,
        sickness,
        has_timer,
    } = ev
    {
        resurrect_request(caster, name, sickness, has_timer, &mut death_net);
    }
}

fn on_spirit_healer_confirm(In(ev): In<SessionEvent>, mut death_net: ResMut<DeathNet>) {
    if let SessionEvent::SpiritHealerConfirm { npc } = ev {
        spirit_healer_confirm(npc, &mut death_net);
    }
}

fn on_durability_damage_death(In(ev): In<SessionEvent>, mut log: ResMut<crate::ui_chat::ChatLog>) {
    if let SessionEvent::DurabilityDamageDeath = ev {
        durability_damage_death(&mut log);
    }
}

fn on_move_mode(
    In(ev): In<SessionEvent>,
    self_guid: Res<SelfGuid>,
    mut death_net: ResMut<DeathNet>,
    mut out: MessageWriter<MoveModeMessage>,
) {
    if let SessionEvent::MoveMode {
        guid,
        counter,
        mode,
        apply,
    } = ev
    {
        move_mode(
            guid,
            counter,
            mode,
            apply,
            &self_guid,
            &mut death_net,
            &mut out,
        );
    }
}

/// The death stores are session-scoped; a reconnect while dead re-sends the reclaim delay.
fn on_session_end(In(_): In<SessionEvent>, mut death_net: ResMut<DeathNet>) {
    *death_net = DeathNet::default();
}

/// Our corpse streaming in: latch its guid for the reclaim send. The reference's latch
/// `[0xb4e328]` has one writer, `0x4920d0`, gated on `CORPSE_FIELD_OWNER == me` and
/// `CORPSE_FIELD_FLAGS` bit 0 clear, so bones never arm it; `RetrieveCorpse` sends whatever is in
/// the latch.
pub(crate) fn note_corpse(
    guid: u64,
    kind: EntityKind,
    fields: &ObjectFields,
    self_guid: &SelfGuid,
    death_net: &mut DeathNet,
) {
    if kind == EntityKind::Corpse
        && fields.corpse_owner() == self_guid.0
        && !fields.corpse_is_bones()
    {
        death_net.corpse_guid = Some(guid);
    }
}

/// The same latch, re-asked when `CORPSE_FIELD_FLAGS` changes under a live guid, as the
/// reference's `FLAGS` mirror handler `0x5d6d60` does, so an in-place flip to bones drops it.
pub(crate) fn recheck_corpse(
    guid: u64,
    fields: &ObjectFields,
    self_guid: &SelfGuid,
    death_net: &mut DeathNet,
) {
    // A delta carries only what changed: an absent FLAGS field is not a clear bit.
    let Some(flags) = fields.corpse_flags_present() else {
        return;
    };
    let mine = fields.corpse_owner() == self_guid.0 || death_net.corpse_guid == Some(guid);
    if !mine {
        return;
    }
    if flags & 0x01 != 0 {
        if death_net.corpse_guid == Some(guid) {
            death_net.corpse_guid = None;
        }
    } else if fields.corpse_owner() == self_guid.0 {
        death_net.corpse_guid = Some(guid);
    }
}

/// The corpse-to-bones swap destroys the corpse object; a stale guid must not ride a reclaim.
pub(crate) fn forget_corpse(guid: u64, death_net: &mut DeathNet) {
    if death_net.corpse_guid == Some(guid) {
        death_net.corpse_guid = None;
    }
}

/// `MSG_CORPSE_QUERY`'s answer; a not-found, asked or pushed at bones conversion, drops the marker.
fn corpse_query(
    found: bool,
    display_map: i32,
    position: [f32; 3],
    corpse_map: u32,
    death_net: &mut DeathNet,
) {
    death_net.corpse = found.then_some(CorpsePoint {
        display_map,
        position,
        corpse_map,
    });
}

/// `SMSG_CORPSE_RECLAIM_DELAY`, anchored at arrival on the clock the feed reads. The 1.12 client
/// re-fires the corpse-range events on each (`0x4962d0`), hence the generation bump.
fn corpse_reclaim_delay(delay_ms: u32, now_secs: f64, death_net: &mut DeathNet) {
    death_net.reclaim_at = Some(now_secs + f64::from(delay_ms) / 1000.0);
    death_net.reclaim_generation = death_net.reclaim_generation.wrapping_add(1);
}

/// `SMSG_RESURRECT_REQUEST`, the RESURRECT popup's data.
fn resurrect_request(
    caster: u64,
    name: String,
    sickness: bool,
    has_timer: bool,
    death_net: &mut DeathNet,
) {
    death_net.resurrect = Some(ResurrectOffer {
        caster,
        name,
        sickness,
        has_timer,
    });
    death_net.resurrect_generation = death_net.resurrect_generation.wrapping_add(1);
}

/// `SMSG_SPIRIT_HEALER_CONFIRM`: the healer's gossip re-sends it on every ask, and the reference
/// fires `CONFIRM_XP_LOSS` per arrival.
fn spirit_healer_confirm(npc: u64, death_net: &mut DeathNet) {
    death_net.ask_spirit_healer(npc);
}

/// `SMSG_DURABILITY_DAMAGE_DEATH`, the 10% death durability loss: a combat-log line at chat type
/// `0x19` `COMBAT_MISC_INFO` with no arguments (`0x628e60`), not an error; the body is ignored.
fn durability_damage_death(log: &mut crate::ui_chat::ChatLog) {
    log.push_combat(crate::ui_chat::combat::PendingCombat {
        kind: crate::ui_chat::ChatEventKind::CombatMiscInfo,
        family: crate::ui_chat::combat::DURABILITYDAMAGE_DEATH,
        variant: crate::ui_chat::combat::Variant::OtherOther,
        subject: 0,
        object: 0,
        fills: crate::ui_chat::combat::Fills::default(),
        named: crate::ui_chat::combat::Named::Ready,
        tries: 0,
    });
}

/// The acked movement modes, root, water-walk, feather-fall and hover, addressed only to our own
/// mover and applied by the controller ([`crate::player::wire_in`]); water-walk is also mirrored
/// into [`DeathNet`] as a ghost cue.
fn move_mode(
    guid: u64,
    counter: u32,
    mode: MoveMode,
    apply: bool,
    self_guid: &SelfGuid,
    death_net: &mut DeathNet,
    out: &mut MessageWriter<MoveModeMessage>,
) {
    if self_guid.0 != Some(guid) {
        return;
    }
    if mode == MoveMode::WaterWalk {
        death_net.water_walk = apply;
    }
    out.write(MoveModeMessage {
        guid,
        counter,
        mode,
        apply,
    });
}
