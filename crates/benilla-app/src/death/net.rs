//! The death packet handlers filling [`DeathNet`], each signal they raise on the dialogs' lane at
//! the packet's place, the movement-mode forward for our own mover, and the corpse-guid latch's
//! hooks on the object lifecycle, called from the drain's object arms.

use benilla_protocol::{EntityKind, MoveMode, ObjectFields, SessionEvent, SessionEventKind};
use benilla_ui::script::ScriptValue;
use bevy::prelude::*;

use super::{CorpsePoint, DeathLatches, DeathNet, ResurrectOffer};
use crate::net::{MoveModeMessage, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_dialog_verbs::DialogLane;

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

/// `MSG_CORPSE_QUERY`'s two arms of `0x48f690`: a found corpse fills the cache
/// (`0x48f7d1` → `0x492010`); a not-found resets it (`0x48f800`) and lowers the range latch
/// (`0x48f807` → `0x4920f0(0)`), whose `CORPSE_OUT_OF_RANGE` signals at the packet when it was set.
fn on_corpse_query(
    In(ev): In<SessionEvent>,
    mut death_net: ResMut<DeathNet>,
    mut latches: ResMut<DeathLatches>,
    mut lane: ResMut<DialogLane>,
) {
    if let SessionEvent::CorpseQuery {
        found,
        display_map,
        position,
        corpse_map,
    } = ev
    {
        corpse_query(found, display_map, position, corpse_map, &mut death_net);
        if !found {
            if let Some(signal) = latches.set_in_range(false, None) {
                signal.queue(&mut lane);
            }
        }
    }
}

/// `SMSG_CORPSE_RECLAIM_DELAY` (`0x48f828` → `SetCorpseReclaimDelay 0x4962d0`): the delay stored,
/// then the in-range event again while the latch is set (`0x4962ec`-`0x49630d`), at the packet, so
/// `RECOVER_CORPSE` re-shows over the new delay.
fn on_corpse_reclaim_delay(
    In(ev): In<SessionEvent>,
    mut death_net: ResMut<DeathNet>,
    latches: Res<DeathLatches>,
    mut lane: ResMut<DialogLane>,
    real_clock: Res<Time<Real>>,
) {
    if let SessionEvent::CorpseReclaimDelay { delay_ms } = ev {
        corpse_reclaim_delay(delay_ms, real_clock.elapsed_secs_f64(), &mut death_net);
        if latches.corpse_in_range {
            super::CorpseSignal::in_range(death_net.corpse.as_ref()).queue(&mut lane);
        }
    }
}

/// `ReadResurrectRequest 0x5e7bc0`: the offer stored (`0x5e7c13`-`0x5e7c2d`); with the active
/// player held (`0x5e7c4a`), a name on the wire shows it at once (`0x5e7c56`), else the caster's
/// name is looked up with the callback `0x5e7ca0` (`0x5e7c82`), a cached one showing it at once.
fn on_resurrect_request(
    In(ev): In<SessionEvent>,
    mut death_net: ResMut<DeathNet>,
    names: Res<crate::names::NameCache>,
    net: Res<crate::net::NetCommands>,
    me: Query<&ObjectStore, With<SelfPlayer>>,
    mut lane: ResMut<DialogLane>,
) {
    if let SessionEvent::ResurrectRequest {
        caster,
        name,
        sickness,
        has_timer,
    } = ev
    {
        resurrect_request(caster, name.clone(), sickness, has_timer, &mut death_net);
        let Ok(me) = me.single() else {
            return;
        };
        let name = if name.is_empty() {
            names
                .resolve_player(caster, Ask::Event(&net))
                .map(str::to_owned)
        } else {
            Some(name)
        };
        match name {
            Some(name) => show_resurrect_request(&me.0, &name, &mut lane),
            None => {
                if let Some(offer) = death_net.resurrect.as_mut() {
                    offer.waits_name = true;
                }
            }
        }
    }
}

/// The callback `0x5e7ca0` for `guid`'s answer, the offer's caster whose name it waited on: a found
/// name shows the offer while the active player is held (`0x5e7d02` → `0x5e7d09`), a refused one
/// forgets it (`0x5e7d19`-`0x5e7d2b`).
pub(crate) fn resurrecter_named(
    death_net: &mut DeathNet,
    me: Option<&ObjectStore>,
    lane: &mut DialogLane,
    guid: u64,
    name: Option<&str>,
) {
    let Some(offer) = death_net
        .resurrect
        .as_mut()
        .filter(|o| o.waits_name && o.caster == guid)
    else {
        return;
    };
    offer.waits_name = false;
    match name.filter(|n| !n.is_empty()) {
        Some(name) => {
            if let Some(me) = me {
                show_resurrect_request(&me.0, name, lane);
            }
        }
        None => death_net.resurrect = None,
    }
}

/// `ShowResurrectRequest 0x5ded50`: `RESURRECT_REQUEST` with a non-empty `name` (`0x5ded5b`)
/// while the active player is dead or a ghost (`0x5ded60` → `0x605f30`).
fn show_resurrect_request(me: &ObjectFields, name: &str, lane: &mut DialogLane) {
    if !name.is_empty() && me.is_dead_or_ghost() {
        lane.event(
            "RESURRECT_REQUEST",
            vec![ScriptValue::Str(name.to_string())],
        );
    }
}

/// `SMSG_SPIRIT_HEALER_CONFIRM`'s arm of the notice handler (`0x5e38c0` → `0x5df730`): the
/// healer's gossip re-sends it on every ask, and each that passes the ask's gates signals at its
/// packet.
fn on_spirit_healer_confirm(
    In(ev): In<SessionEvent>,
    mut ask: super::SpiritHealerAsk,
    mut lane: ResMut<DialogLane>,
) {
    if let SessionEvent::SpiritHealerConfirm { npc } = ev {
        if let Some(cost) = ask.ask(npc) {
            lane.event("CONFIRM_XP_LOSS", vec![ScriptValue::Int(cost)]);
        }
    }
}

fn on_durability_damage_death(In(ev): In<SessionEvent>, log: Res<crate::ui_chat::ChatLog>) {
    if let SessionEvent::DurabilityDamageDeath = ev {
        durability_damage_death(&log);
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

/// The death stores and latches are session-scoped; a reconnect while dead re-sends the reclaim
/// delay, and a relog as a ghost must find no mirror, or the corpse query is never re-sent.
pub(super) fn on_session_end(
    In(_): In<SessionEvent>,
    mut death_net: ResMut<DeathNet>,
    mut latches: ResMut<DeathLatches>,
) {
    *death_net = DeathNet::default();
    *latches = DeathLatches::default();
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
    death_net: &mut impl std::ops::DerefMut<Target = DeathNet>,
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

/// `SMSG_CORPSE_RECLAIM_DELAY`, anchored at arrival on the clock the getters read.
fn corpse_reclaim_delay(delay_ms: u32, now_secs: f64, death_net: &mut DeathNet) {
    death_net.reclaim_at = Some(now_secs + f64::from(delay_ms) / 1000.0);
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
        waits_name: false,
    });
}

/// `SMSG_DURABILITY_DAMAGE_DEATH`, the 10% death durability loss: a combat-log line at chat type
/// `0x19` `COMBAT_MISC_INFO` with no arguments (`0x628e60`), not an error; the body is ignored.
fn durability_damage_death(log: &crate::ui_chat::ChatLog) {
    log.push_combat(crate::ui_chat::combat::PendingCombat {
        kind: crate::ui_chat::ChatEventKind::CombatMiscInfo,
        family: crate::ui_chat::combat::DURABILITYDAMAGE_DEATH,
        variant: crate::ui_chat::combat::Variant::OtherOther,
        subject: 0,
        object: 0,
        fills: crate::ui_chat::combat::Fills::default(),
        named: crate::ui_chat::combat::Named::Ready,
        tries: 0,
        word: None,
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
