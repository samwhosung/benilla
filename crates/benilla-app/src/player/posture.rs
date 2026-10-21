//! Posture: the stand state (the `/sit` family and `X`) and the sheath toggle (`Z`), one
//! mechanism in the reference. A stand state other than 0 or 2 stows drawn weapons through the
//! setter `Z` drives (2 inferred to be the chair sit, vmangos's `UNIT_STAND_STATE_SIT_CHAIR`), and
//! the toggle refuses on the stand state this frame committed. Both refuse silently, with no
//! packet, so the `sit` trace tag ([`super::move_trace::posture`]) records each commit and refusal.

use benilla_protocol::ObjectFields;
use bevy::prelude::*;

use super::{move_trace, state, BodyQuery, ClientCommand, NetCommands, Player, StandStateRequest};

/// What the posture verbs read of the player beyond its body's components.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PlayerReads {
    /// Our own control lost ([`super::OwnControlLost`]): `IsClientControlled` (`0x5fa5f0`) reads
    /// the unit's `[+0xc58] & 0x400`, which `0x5fa600` writes beside that flag for our character.
    pub(super) control_lost: bool,
    /// The body holds a precast spell ([`crate::creature_anim::Precast`], `+0xc8c`).
    pub(super) precast: bool,
}

/// Runs this frame's stand-state decision and sheath toggle, and returns the committed stand
/// state: the local commit over the server's echoed byte, which the pose and sheath guard read.
pub(super) fn update(
    player: &mut Player,
    body: &BodyQuery,
    binds: &crate::bindings::BindingsState,
    net: &NetCommands,
    commands: &mut Commands,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    loot_close: &mut crate::ui_loot::LootMoveStart,
    asks: &mut MessageReader<StandStateRequest>,
    server: &mut MessageReader<super::ServerStandState>,
    reads: PlayerReads,
    classes: Option<&benilla_formats::ChrClasses>,
    moving: bool,
    turned: bool,
) -> u8 {
    // The echo into `UNIT_FIELD_BYTES_1` drives every observer's pose; `stand_pending`, the local
    // commit ([`apply_locally`]), overlays it until it lands.
    let (stand_byte, gates) = body
        .single()
        .ok()
        .and_then(|(.., store, _, _, _, _, _)| {
            store.map(|s| (s.0.unit_stand_state(), stand_gates(&s.0, reads.precast)))
        })
        .unwrap_or((
            0,
            state::StandGates {
                precast: reads.precast,
                ..default()
            },
        ));
    if player.stand_pending == Some(stand_byte) {
        player.stand_pending = None; // the echo landed
    }
    // `SMSG_STANDSTATE_UPDATE` reaches the local apply directly, with no refusal gate and no
    // `CMSG_STANDSTATECHANGE` back, before the volunteered logic below; the last packet wins.
    if let Some(s) = server.read().last().map(|m| m.state) {
        let stand_now = player.stand_pending.unwrap_or(stand_byte);
        move_trace::posture("server", s, stand_now, player.move_flags());
        apply_locally(player, s, stand_now, body, commands, sheath, loot_close);
    }
    let stand_state = player.stand_pending.unwrap_or(stand_byte);
    // The queued asks (the `/sit` family) ran in the reference's message pass, before the X key
    // is read; the last writer wins, and all land on the single commit below.
    let mut request_stand = asks.read().last().map(|r| r.state);
    if binds.fired(crate::bindings::Input::SitOrStand) {
        request_stand = Some(u8::from(stand_state == 0));
    }
    // Movement stands us up, volunteered, as the server never does: translation, a keyboard turn
    // and jump reach the stand wrapper `0x60be30(0)` from any nonzero state; a left-drag orbit
    // does not, nor a right-drag turn (`0x514f50` skips its stand arm while the button is held),
    // though a release within 200 ms is a right-click, whose action stands you. A knockback's move
    // event stands us too (`0x60e139`: `GetStandState` `0x60be50`, then `0x60be30(0)`), read off
    // the latch [`super::wire_in::apply_server_moves`] armed this frame, as the mover runs later.
    let knocked_out_of_it = player.knockback.is_some();
    if (moving || turned || knocked_out_of_it || binds.fired(crate::bindings::Input::Jump))
        && stand_state != 0
        && request_stand.is_none()
    {
        request_stand = Some(0);
    }
    // `SetStandState` `0x5ed430`'s gate ([`state::stand_state_refused`]), silent and before the
    // packet: no change at all while mounted, on a taxi, dead, channelling or casting, and no
    // sitting while translating or swimming. It covers the posture emotes too, whose `Emotes.dbc`
    // gate has no swim bit. The flags word is the live outbound one, a frame old, the
    // `[[this+0x118]+0x40]` the cast gates read.
    if let Some(s) =
        request_stand.filter(|&s| state::stand_state_refused(gates, player.move_flags(), s))
    {
        debug!(
            "stand state {s} refused ({gates:?}, move flags {:#x} — the client's `0x5ed430` gate)",
            player.move_flags()
        );
        move_trace::posture("REFUSED", s, stand_state, player.move_flags());
        request_stand = None;
    }
    if let Some(s) = request_stand.filter(|&s| s != stand_state) {
        move_trace::posture("commit", s, stand_state, player.move_flags());
        // `0x5ed430` sends at `0x5ed501`, then calls the local apply at `0x5ed53f`.
        let _ = net.0.send(ClientCommand::StandStateChange {
            state: u32::from(s),
        });
        apply_locally(player, s, stand_state, body, commands, sheath, loot_close);
    }
    let stand_now = player.stand_pending.unwrap_or(stand_byte);
    // The sheath toggle (Z) cycles the anim layer's committed sheath state through its one setter
    // ([`crate::creature_anim::SheathRequest`]), which sends `CMSG_SETSHEATHED`; only it plays the
    // ceremony (`bInstant = 0` in `ToggleSheath` `0x5eb480`). No body model yet drops the press.
    if binds.fired(crate::bindings::Input::ToggleSheath) {
        if let Ok((e, _, _, _, Some((drv, committed)), store, engaged, _, _, wielded, _)) =
            body.single()
        {
            let toggle = ToggleGates {
                control_lost: reads.control_lost,
                engaged,
                precast: reads.precast,
                seated: stand_now != 0,
                mid_ceremony: drv.sheath_ceremony_active(),
            };
            if toggle_refused(store.map(|s| &s.0), toggle) {
                debug!("sheath toggle refused ({toggle:?})");
            } else {
                // Melee, ranged, stowed, skipping what is not worn; `None` makes no call at all.
                let w = wielded.copied().unwrap_or_default();
                let worn = toggle_worn(&w, store.map(|s| &s.0), classes);
                let next =
                    crate::creature_anim::toggle_sheath_next(committed.state().unwrap_or(0), worn);
                if let Some(state) = next {
                    sheath.write(crate::creature_anim::SheathRequest {
                        entity: e,
                        state,
                        ceremony: true,
                    });
                }
            }
        }
    }

    stand_now
}

/// [`state::StandGates`] off the body's descriptor and cast state.
fn stand_gates(fields: &ObjectFields, precast: bool) -> state::StandGates {
    state::StandGates {
        mounted: fields.unit_mount_display_id() != 0,
        on_taxi: fields.unit_flags() & super::UNIT_FLAG_TAXI_FLIGHT != 0,
        // Health 0 or the feign flag, without `unit_reads_dead`'s stand state 7 (`0x5ed4a9`–
        // `0x5ed4bd`).
        reads_dead: fields.unit_is_dead() || fields.unit_dynflag_dead(),
        channelling: fields.unit_channel_spell() != 0,
        precast,
    }
}

/// What `ToggleSheath` (`0x5eb480`) reads of the player ahead of its cycle, past the active-player
/// test, besides the descriptor [`toggle_refused`] reads; each refuses silently, with no call and
/// no packet. Its model tests (`0x5eb542`, `0x5eb580`) are the setter's executor's, which holds
/// the model ([`crate::creature_anim::SheathRequest`]).
#[derive(Clone, Copy, Debug, Default)]
struct ToggleGates {
    /// `IsClientControlled` (`0x5eb4d1`) is clear.
    control_lost: bool,
    /// An attack target, `[+0xc48]` (`0x5eb513`).
    engaged: bool,
    /// A precast spell, `[+0xd58] & 0x400` (`0x5eb49f`) or `[+0xc8c]` (`0x5eb52b`).
    precast: bool,
    /// A stand state above 0 (`0x5eb553`).
    seated: bool,
    /// An arm still plays a sheathe animation (`0x5eb5a0`–`0x5eb5e2`).
    mid_ceremony: bool,
}

/// Whether the toggle refuses: on `gates`, and off the descriptor on a mount (`0x5eb4e4`), the
/// stun flag (`0x5eb4f2`), health at or below 0 (`0x5eb502`) and a channel (`0x5eb48e`).
fn toggle_refused(fields: Option<&ObjectFields>, gates: ToggleGates) -> bool {
    let mounted = fields.is_some_and(|f| f.unit_mount_display_id() != 0);
    let stunned = fields.is_some_and(|f| f.unit_flags() & super::UNIT_FLAG_STUNNED != 0);
    let dead = fields.is_some_and(ObjectFields::unit_is_dead);
    let channelling = fields.is_some_and(|f| f.unit_channel_spell() != 0);
    gates.control_lost
        || mounted
        || stunned
        || dead
        || gates.engaged
        || channelling
        || gates.precast
        || gates.seated
        || gates.mid_ceremony
}

/// What the toggle cycles over: a hand holding a weapon and a ranged candidate, as
/// `GetWeapon(slot, 0)` reads them (`0x5eb5f0`, `0x5eb600`, `0x5eb610`: a disarmed hand is not
/// worn), the ranged one kept only for a class with a ranged sheathe state (`0x5eb616`-`0x5eb640`,
/// [`benilla_formats::ChrClasses::has_ranged_sheath`]). Without the table, it is kept.
fn toggle_worn(
    w: &crate::creature_anim::Wielded,
    fields: Option<&ObjectFields>,
    classes: Option<&benilla_formats::ChrClasses>,
) -> (bool, bool) {
    // The class byte `[+0x79]`, `UNIT_FIELD_BYTES_0` byte 1.
    let class = u32::from(fields.and_then(ObjectFields::unit_class).unwrap_or(0));
    (
        w.armed_main().is_some() || w.armed_off().is_some(),
        w.ranged.is_some() && classes.is_none_or(|t| t.has_ranged_sheath(class)),
    )
}

/// The stand state's local apply, the reference's `0x6127b0`, reached from `SetStandState` after
/// its send and from `SMSG_STANDSTATE_UPDATE` (`0x603e50`), doing what [`local_apply`] decides.
/// Not built: the input resume on a stand (`0x5f07ba`) and the follow-camera re-acquire each call
/// (`0x48ec90`).
fn apply_locally(
    player: &mut Player,
    s: u8,
    stand_now: u8,
    body: &BodyQuery,
    commands: &mut Commands,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    loot_close: &mut crate::ui_loot::LootMoveStart,
) {
    let body = body.single().ok();
    let drawn = body
        .and_then(|(_, _, _, _, drv, ..)| drv)
        .and_then(|(_, committed)| committed.state())
        .unwrap_or(0)
        != 0;
    let apply = local_apply(s, stand_now, drawn);
    if apply.predict {
        player.stand_pending = Some(s);
    }
    if apply.leave {
        // The same close as the movement start's (`0x60e9dd`), under the same `0x6126b0`.
        loot_close.0 = true;
        commands.run_system_cached(stop_attack_for_stand_state);
    }
    if let (true, Some((e, ..))) = (apply.stow, body) {
        sheath.write(crate::creature_anim::SheathRequest {
            entity: e,
            state: 0,
            ceremony: false,
        });
    }
}

/// What the local apply (`0x6127b0`) does for state `s` over the committed `stand_now`, with the
/// body's weapons `drawn` or not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LocalApply {
    /// `0x5f0790` writes the prediction `[player+0x1d68]`, only on a change (`0x5f0799`).
    predict: bool,
    /// That change, to any state but 0, ends the loot window (`0x48f200(1, 1, 0)` at `0x5f07d5`,
    /// while `0x6126b0` holds) and the attack (`0x5ecac0` at `0x5f07e7`, while `0x60ecb0` holds).
    leave: bool,
    /// Any call stows a drawn weapon for a state but 0 or 2 (`0x6127cc`, `0x611cf0(0, 1, 1)`).
    stow: bool,
}

fn local_apply(s: u8, stand_now: u8, drawn: bool) -> LocalApply {
    let predict = s != stand_now;
    LocalApply {
        predict,
        leave: predict && s != 0,
        stow: drawn && s != 0 && s != 2,
    }
}

/// StopAttack (`0x5ecac0`) as the stand state's local apply calls it, on our own attack target
/// (`0x60ecb0`): `CMSG_ATTACKSTOP` and the queued strike's cancel.
fn stop_attack_for_stand_state(
    mut slots: crate::spell::CastSlots,
    net: Res<NetCommands>,
    me: Query<(Entity, Has<crate::creature_anim::Engaged>), With<crate::net::SelfPlayer>>,
    mut word: ResMut<crate::creature_anim::AttackStopInFlight>,
) {
    let (me, engaged) = me
        .iter()
        .next()
        .map_or((None, false), |(me, engaged)| (Some(me), engaged));
    crate::creature_anim::stop_attack_local(me, engaged, &mut word, &mut slots.core(&net));
}

#[cfg(test)]
mod tests;
