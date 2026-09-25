//! Plays a bridged `SMSG_EMOTE` through the one-shot player ([`super::EmoteAnim`]), its
//! `Emotes.dbc` id resolved through the shared catalog ([`EmoteSounds`]). A one-shot `/`-emote
//! (`EmoteType` 0) sends `SMSG_TEXT_EMOTE` and `SMSG_EMOTE` from one handler for the same id
//! (vmangos `ChatHandler.cpp:738`, `Unit.cpp:1789-1792`), so only `SMSG_EMOTE` animates; a state
//! emote sets `UNIT_NPC_EMOTESTATE` instead and plays as the gait layer's idle.
//!
//! The reference's `SMSG_EMOTE` handler (`0x5e66b0`) never reads `Emotes.dbc`: it suppresses only
//! a sleeping (stand state 3) or swimming performer ([`receive_eligible`]). The send-side gate in
//! `crate::ui_chat` is a separate test, so do not add an `EmoteFlags` check here.

use bevy::prelude::*;

use crate::net::{EmoteKind, EmoteMessage, ObjectStore, RemoteMotion};
use crate::sound::EmoteSounds;

use super::{move_flags, EmoteAnim, Engaged, MovementState};

/// The performer's gate inputs: stand state, move flags (`MovementState` for us, `RemoteMotion`
/// for a remote player; a creature's spline carries no swim bit) and the [`Engaged`] marker.
pub(super) type PerformerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static ObjectStore>,
        Option<&'static MovementState>,
        Option<&'static RemoteMotion>,
        Has<Engaged>,
    ),
>;

/// Route a bridged `SMSG_EMOTE` to the one-shot player, gated on the performer's live state.
pub(super) fn emote_to_anim(
    mut msgs: MessageReader<EmoteMessage>,
    mut out: MessageWriter<EmoteAnim>,
    mut play_seq: ResMut<super::PlaySeq>,
    emotes: Option<Res<EmoteSounds>>,
    units: PerformerQuery,
) {
    let Some(emotes) = emotes else { return };
    for m in msgs.read() {
        let (Some(entity), Some(anim_id)) =
            (m.source, resolve_anim_emote(m.kind, |id| emotes.anim(id)))
        else {
            continue;
        };
        let (store, movement, remote, engaged) =
            units.get(entity).unwrap_or((None, None, None, false));
        if !play_eligible(store, movement, remote, engaged) {
            debug!("emote_anim: suppressed anim {anim_id} for {entity:?}");
            continue;
        }
        out.write(EmoteAnim {
            entity,
            anim_id: anim_id as u16,
            seq: play_seq.next(),
        });
    }
}

/// The posture half of the gate, which both producers apply (`SMSG_EMOTE` at
/// `0x5e6706`/`0x5e66f7`, the gesture dispatcher at `0x60bb52`/`0x60bb61`): only sleep (stand
/// state 3) or swimming suppresses; a seated performer plays, masked to waist-up downstream.
pub(super) fn receive_eligible(stand_state: u8, swimming: bool) -> bool {
    stand_state != 3 && !swimming
}

/// The shared player's half, `0x5fcd20`: no play while channeling (`0x5fcd83`) or in combat
/// (`0x5fcd9d`). Its already-armed test (`0x5fcd5d`) is the driver's same-id dedup, and its
/// `[+0xd58] & 0x400` test (`0x5fcd8e`) reads an anim-state bit with no counterpart here.
fn player_eligible(channeling: bool, in_combat: bool) -> bool {
    !channeling && !in_combat
}

/// The whole gate for one unit, as both producers call it; `engaged` stands for the client's
/// auto-attack target `[unit+0xc48]`.
pub(super) fn play_eligible(
    store: Option<&ObjectStore>,
    movement: Option<&MovementState>,
    remote: Option<&RemoteMotion>,
    engaged: bool,
) -> bool {
    let fields = store.map(|s| &s.0);
    let stand_state = fields.map_or(0, |f| f.unit_stand_state());
    let swimming = movement
        .map(|m| m.flags)
        .or_else(|| remote.map(|r| r.flags))
        .unwrap_or(0)
        & move_flags::SWIMMING
        != 0;
    let channeling = fields.is_some_and(|f| f.unit_channel_spell() != 0);
    let in_combat = engaged || fields.is_some_and(|f| f.unit_flags() & UNIT_FLAGS_COMBAT_BIT != 0);
    receive_eligible(stand_state, swimming) && player_eligible(channeling, in_combat)
}

/// The flag half of the in-combat test `0x60ecd0` (`UNIT_FIELD_FLAGS` bit 11); the other half is
/// the auto-attack target (`0x60ecb0`). vmangos marks combat with `0x80000` and sets `0x800` only
/// on a pet or charmed unit (`Unit.cpp:6170-6173`), so there the attack target carries the gate.
const UNIT_FLAGS_COMBAT_BIT: u32 = 0x800;

/// A `Text` emote's animation arrives as its own `SMSG_EMOTE`.
fn resolve_anim_emote(kind: EmoteKind, lookup: impl Fn(u32) -> Option<u32>) -> Option<u32> {
    match kind {
        EmoteKind::Anim(id) => lookup(id),
        EmoteKind::Text(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_anim_kind_resolves_and_only_through_the_catalog() {
        let lookup = |id: u32| if id == 3 { Some(66) } else { None };
        assert_eq!(resolve_anim_emote(EmoteKind::Anim(3), lookup), Some(66));
        assert_eq!(resolve_anim_emote(EmoteKind::Text(101), lookup), None);
        // An id the catalog lacks (AnimID 0 included) stays `None`.
        assert_eq!(resolve_anim_emote(EmoteKind::Anim(999), lookup), None);
    }

    // ── The receive-side posture gate ──
    #[test]
    fn sleep_suppresses_the_receive_side_anim() {
        assert!(!receive_eligible(3, false));
    }

    #[test]
    fn swimming_suppresses_the_receive_side_anim() {
        assert!(!receive_eligible(0, true));
    }

    #[test]
    fn channeling_or_combat_suppresses_the_play() {
        assert!(
            player_eligible(false, false),
            "idle and out of combat plays"
        );
        assert!(!player_eligible(true, false), "channeling suppresses");
        assert!(!player_eligible(false, true), "in combat suppresses");
    }

    /// The attack-target half of `0x60ecd0` (`[+0xc48]`, from ATTACKSTART to ATTACKSTOP) refuses
    /// on its own: on vmangos the flag half never fires for a fighting player or creature.
    #[test]
    fn an_engaged_unit_refuses_the_play_without_the_flag_bit() {
        assert!(
            play_eligible(None, None, None, false),
            "idle, no store: plays"
        );
        assert!(!play_eligible(None, None, None, true), "engaged: refused");
    }

    #[test]
    fn merely_seated_is_not_suppressed_on_receive() {
        // Unlike the send-side gate, every seated state passes; only sleep does not.
        for stand_state in [0u8, 1, 2, 4, 5, 6, 8] {
            assert!(
                receive_eligible(stand_state, false),
                "stand_state {stand_state}"
            );
        }
    }
}
