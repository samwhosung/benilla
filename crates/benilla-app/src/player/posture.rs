//! Posture: the stand state (the `/sit` family and `X`) and the sheath toggle (`Z`), one
//! mechanism in the reference. A stand state other than 0 or 2 stows drawn weapons through the
//! setter `Z` drives (2 inferred to be the chair sit, vmangos's `UNIT_STAND_STATE_SIT_CHAIR`), and
//! the toggle refuses on the stand state this frame committed. Both refuse silently, with no
//! packet, so the `sit` trace tag ([`super::move_trace::posture`]) records each commit and refusal.

use bevy::prelude::*;

use super::{move_trace, state, BodyQuery, ClientCommand, NetCommands, Player, StandStateRequest};

/// Runs this frame's stand-state decision and sheath toggle, and returns the committed stand
/// state: the local commit over the server's echoed byte, which the pose and sheath guard read.
pub(super) fn update(
    player: &mut Player,
    body: &BodyQuery,
    binds: &crate::bindings::BindingsState,
    net: &NetCommands,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
    asks: &mut MessageReader<StandStateRequest>,
    server: &mut MessageReader<super::ServerStandState>,
    moving: bool,
    turned: bool,
) -> u8 {
    // The echo into `UNIT_FIELD_BYTES_1` drives every observer's pose; `stand_pending`, the local
    // commit ([`apply_locally`]), overlays it until it lands.
    let (stand_byte, reads_dead) = body
        .single()
        .ok()
        .and_then(|(.., store, _, _, _, _, _)| {
            // `SetStandState`'s first guards, health ≤ 0 or `UNIT_DYNAMIC_FLAGS & 0x20` (feigning),
            // without `unit_reads_dead`'s stand state 7 (`0x5ed4a9`–`0x5ed4bd`).
            store.map(|s| {
                (
                    s.0.unit_stand_state(),
                    s.0.unit_is_dead() || s.0.unit_dynflag_dead(),
                )
            })
        })
        .unwrap_or((0, false));
    if player.stand_pending == Some(stand_byte) {
        player.stand_pending = None; // the echo landed
    }
    // `SMSG_STANDSTATE_UPDATE` reaches the local apply directly, with no refusal gate and no
    // `CMSG_STANDSTATECHANGE` back, before the volunteered logic below; the last packet wins.
    if let Some(s) = server.read().last().map(|m| m.state) {
        let stand_now = player.stand_pending.unwrap_or(stand_byte);
        move_trace::posture("server", s, stand_now, player.move_flags());
        apply_locally(player, s, stand_now, body, sheath);
    }
    let stand_state = player.stand_pending.unwrap_or(stand_byte);
    // The queued asks (the `/sit` family) ran in the reference's message pass, before the X key
    // is read; the last writer wins, and all land on the single commit below.
    let mut request_stand = asks.read().last().map(|r| r.state);
    if binds.fired(crate::bindings::cmd::SIT_OR_STAND) {
        request_stand = Some(u8::from(stand_state == 0));
    }
    // Movement stands us up, volunteered, as the server never does: translation, a keyboard turn
    // and jump reach the stand wrapper `0x60be30(0)` from any nonzero state; a left-drag orbit
    // does not, nor a right-drag turn (`0x514f50` skips its stand arm while the button is held),
    // though a release within 200 ms is a right-click, whose action stands you. A knockback's move
    // event stands us too (`0x60e139`: `GetStandState` `0x60be50`, then `0x60be30(0)`), read off
    // the latch [`super::wire_in::apply_server_moves`] armed this frame, as the mover runs later.
    let knocked_out_of_it = player.knockback.is_some();
    if (moving || turned || knocked_out_of_it || binds.fired(crate::bindings::cmd::JUMP))
        && stand_state != 0
        && request_stand.is_none()
    {
        request_stand = Some(0);
    }
    // `SetStandState` `0x5ed430`'s gate ([`state::stand_state_refused`]), silent and before the
    // packet: no sitting while translating or swimming, no change at all while dead. It covers
    // the posture emotes too, whose `Emotes.dbc` gate has no swim bit. The flags word is the live
    // outbound one, a frame old, the `[[this+0x118]+0x40]` the cast gates read.
    if let Some(s) =
        request_stand.filter(|&s| state::stand_state_refused(reads_dead, player.move_flags(), s))
    {
        debug!(
            "stand state {s} refused (dead {reads_dead}, move flags {:#x} — the client's \
             `0x5ed430` gate)",
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
        apply_locally(player, s, stand_state, body, sheath);
    }
    let stand_now = player.stand_pending.unwrap_or(stand_byte);
    // The sheath toggle (Z) cycles the anim layer's committed sheath state through its one setter
    // ([`crate::creature_anim::SheathRequest`]), which sends `CMSG_SETSHEATHED`; only it plays the
    // ceremony (`bInstant = 0` in `ToggleSheath` `0x5eb480`). No body model yet drops the press.
    if binds.fired(crate::bindings::cmd::TOGGLE_SHEATH) {
        if let Ok((e, _, _, _, Some(drv), store, engaged, _, _, wielded, _)) = body.single() {
            // Of `ToggleSheath`'s 12 silent guards: dead, in combat, any nonzero stand state,
            // mid-ceremony (clip 89/90) and mounted; its stunned and channeling guards are not
            // tested here.
            let dead = store.is_some_and(|s| s.0.unit_is_dead());
            let mounted = store.is_some_and(|s| s.0.unit_mount_display_id() != 0);
            let refused =
                dead || mounted || engaged || stand_now != 0 || drv.sheath_ceremony_active();
            if refused {
                debug!(
                    "sheath toggle refused (dead {dead}, mounted {mounted}, engaged {engaged}, \
                     stand {stand_now}, mid-ceremony {})",
                    drv.sheath_ceremony_active()
                );
            } else {
                // Melee, ranged, stowed, skipping what is not worn; `None` makes no call at all.
                let w = wielded.copied().unwrap_or_default();
                // `GetWeapon(slot, 0)` at `0x5eb5f0`, `0x5eb600` and `0x5eb610`: a disarmed hand
                // is not worn.
                let worn = (
                    w.armed_main().is_some() || w.armed_off().is_some(),
                    w.ranged.is_some(),
                );
                let next =
                    crate::creature_anim::toggle_sheath_next(drv.sheath_state().unwrap_or(0), worn);
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

/// The stand state's local apply, the reference's `0x6127b0`, reached from `SetStandState` after
/// its send and from `SMSG_STANDSTATE_UPDATE` (`0x603e50`); it sends nothing. A change writes the
/// prediction (`[player+0x1d68]`); any call stows a drawn weapon for a state but 0 or 2
/// (`0x6127cc`). Not built: its loot release and attack stop on a sit (`0x5f0790` → `0x48f200`,
/// `0x5ecac0`), movement re-run on a stand, and follow-camera re-acquire each call (`0x48ec90`).
fn apply_locally(
    player: &mut Player,
    s: u8,
    stand_now: u8,
    body: &BodyQuery,
    sheath: &mut MessageWriter<crate::creature_anim::SheathRequest>,
) {
    predict(&mut player.stand_pending, s, stand_now);
    if s != 0 && s != 2 {
        if let Ok((e, _, _, _, drv, _, _, _, _, _, _)) = body.single() {
            if drv.and_then(|d| d.sheath_state()).unwrap_or(0) != 0 {
                sheath.write(crate::creature_anim::SheathRequest {
                    entity: e,
                    state: 0,
                    ceremony: false,
                });
            }
        }
    }
}

/// `0x5f0790`'s early-out (`0x5f0799`): the predicted stand state is written only on a change.
/// Returns whether it was.
fn predict(pending: &mut Option<u8>, s: u8, stand_now: u8) -> bool {
    if s == stand_now {
        return false;
    }
    *pending = Some(s);
    true
}

#[cfg(test)]
mod setter_tests {
    use super::predict;

    #[test]
    fn the_local_half_writes_the_prediction_only_on_a_change() {
        // Seated by the server (a drink) while the echo still says standing: predicted.
        let mut pending = None;
        assert!(predict(&mut pending, 1, 0));
        assert_eq!(pending, Some(1));
        // vmangos's same-state re-send (its camera re-acquire): nothing written.
        let mut pending = None;
        assert!(!predict(&mut pending, 0, 0));
        assert_eq!(pending, None);
        // A stand from the server while we predicted a sit: the newer state wins.
        let mut pending = Some(1);
        assert!(predict(&mut pending, 0, 1));
        assert_eq!(pending, Some(0));
    }
}
