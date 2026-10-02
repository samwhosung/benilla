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

use super::{move_flags, select, EmoteAnim, Engaged, MovementState};

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
        if !play_eligible(store, movement, remote, engaged, anim_id) {
            debug!("emote_anim: suppressed anim {anim_id} for {entity:?}");
            continue;
        }
        out.write(EmoteAnim {
            entity,
            anim_id: anim_id as u16,
            seq: play_seq.next(),
            via_player: true,
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
/// (`0x5fcd9d`), and LiftOff (192) and Land (200) skip the combat test alone (`0x5fcd5f`/`0x5fcd67`
/// set the flag that `0x5fcd90` branches past `0x60ecd0` on; the channel test at `0x5fcd83` runs
/// first). Its already-armed test (`0x5fcd5d`) is the driver's same-id dedup, and its
/// `[+0xd58] & 0x400` test (`0x5fcd8e`, a cast in progress) has no counterpart here.
fn player_eligible(anim_id: u32, channeling: bool, in_combat: bool) -> bool {
    let exempt = anim_id == u32::from(select::LIFT_OFF) || anim_id == u32::from(select::LAND);
    !channeling && (exempt || !in_combat)
}

/// The whole gate for one unit, as both producers call it; `engaged` stands for the client's
/// auto-attack target `[unit+0xc48]`.
pub(super) fn play_eligible(
    store: Option<&ObjectStore>,
    movement: Option<&MovementState>,
    remote: Option<&RemoteMotion>,
    engaged: bool,
    anim_id: u32,
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
    receive_eligible(stand_state, swimming) && player_eligible(anim_id, channeling, in_combat)
}

/// Our own unit's inputs to `DoEmote`'s local-play gate `0x5fd680`.
#[derive(Clone, Copy, Default)]
pub(crate) struct LocalPlay {
    /// The shared dead test `0x605f90`: health 0, `UNIT_DYNFLAG_DEAD` or stand state 7.
    pub(crate) reads_dead: bool,
    pub(crate) stand_state: u8,
    pub(crate) move_flags: u32,
    pub(crate) vertical_speed: f32,
    /// On a flying server spline (a taxi), which exempts the turn-shuffle test (`0x5fce47`).
    pub(crate) flying: bool,
    /// The loot-kneel latch with its kneeling target (`0x6126b0` and `0x612710`).
    pub(crate) loot_kneel: bool,
    pub(crate) channeling: bool,
    /// The auto-attack target `[+0xc48]` alone (`0x60ecb0`), not the combat flag bit.
    pub(crate) engaged: bool,
}

/// Move flags that void the turn-shuffle test: HOVER, SWIMMING and `0x800` (`0x5fce5d`).
const SHUFFLE_VOID: u32 = move_flags::HOVER | move_flags::SWIMMING | 0x800;

/// `0x5fd680`, the gate on `DoEmote`'s local play (`0x5ef5b6`): the emote plays at once only while
/// a direction key is held, and every other case waits for the server's `SMSG_EMOTE`. It refuses
/// the dead (`0x605f90`), a unit both jumping and falling far (`0x5fd6a9`), a jump with vertical
/// speed (`0x5fd6c5`), a kneeling looter (`0x5fd6e7`), a channel (`0x5fd701`), an auto-attack
/// target (`0x5fd721`), a turn shuffle (`0x5fd72c`) and any stand state but 0 (`0x5fd74d`, and
/// the cached `[+0xc1c]` at `0x5fd757`, which tracks it). The `[+0xd58]` test at `0x5fd711` reads
/// a cast-in-progress bit with no counterpart here, and the ranged auto-repeat test (`0x5fd735`)
/// needs sheath state 2, which the stow `DoEmote` runs first (`0x611cf0(0)`) has cleared.
pub(crate) fn local_play_eligible(i: &LocalPlay) -> bool {
    let f = i.move_flags;
    let turning = f & (move_flags::TURN_LEFT | move_flags::TURN_RIGHT) != 0;
    let shuffling = turning && !i.flying && f & SHUFFLE_VOID == 0;
    !i.reads_dead
        && !(f & move_flags::FALLING != 0 && f & move_flags::FALLING_FAR != 0)
        && (f & move_flags::FALLING == 0 || i.vertical_speed == 0.0)
        && f & move_flags::ANY_MOVE != 0
        && !i.loot_kneel
        && !i.channeling
        && !i.engaged
        && !shuffling
        && i.stand_state == 0
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
            player_eligible(66, false, false),
            "idle and out of combat plays"
        );
        assert!(!player_eligible(66, true, false), "channeling suppresses");
        assert!(!player_eligible(66, false, true), "in combat suppresses");
    }

    /// `0x5fcd5f`/`0x5fcd67`: LiftOff (192) and Land (200) skip the combat test, not the channel.
    #[test]
    fn lift_off_and_land_play_in_combat_but_not_while_channeling() {
        assert!(player_eligible(192, false, true), "LiftOff in combat");
        assert!(player_eligible(200, false, true), "Land in combat");
        assert!(!player_eligible(66, false, true), "any other id refuses");
        assert!(!player_eligible(193, false, true), "the neighbours refuse");
        assert!(
            !player_eligible(192, true, true),
            "channeling still refuses"
        );
    }

    /// The attack-target half of `0x60ecd0` (`[+0xc48]`, from ATTACKSTART to ATTACKSTOP) refuses
    /// on its own: on vmangos the flag half never fires for a fighting player or creature.
    #[test]
    fn an_engaged_unit_refuses_the_play_without_the_flag_bit() {
        assert!(
            play_eligible(None, None, None, false, 66),
            "idle, no store: plays"
        );
        assert!(
            !play_eligible(None, None, None, true, 66),
            "engaged: refused"
        );
        assert!(
            play_eligible(None, None, None, true, 192),
            "engaged LiftOff"
        );
        assert!(play_eligible(None, None, None, true, 200), "engaged Land");
    }

    fn moving() -> LocalPlay {
        LocalPlay {
            move_flags: move_flags::FORWARD,
            ..Default::default()
        }
    }

    #[test]
    fn a_local_emote_plays_only_while_a_direction_key_is_held() {
        assert!(local_play_eligible(&moving()));
        for dir in [
            move_flags::BACKWARD,
            move_flags::STRAFE_LEFT,
            move_flags::STRAFE_RIGHT,
        ] {
            let i = LocalPlay {
                move_flags: dir,
                ..moving()
            };
            assert!(local_play_eligible(&i), "{dir:#x}");
        }
        assert!(
            !local_play_eligible(&LocalPlay::default()),
            "standing still"
        );
        let turn_only = LocalPlay {
            move_flags: move_flags::TURN_LEFT,
            ..Default::default()
        };
        assert!(!local_play_eligible(&turn_only), "turning in place");
    }

    #[test]
    fn each_local_play_gate_refuses_on_its_own() {
        let base = moving();
        let refused = [
            LocalPlay {
                reads_dead: true,
                ..base
            },
            LocalPlay {
                stand_state: 1,
                ..base
            },
            LocalPlay {
                loot_kneel: true,
                ..base
            },
            LocalPlay {
                channeling: true,
                ..base
            },
            LocalPlay {
                engaged: true,
                ..base
            },
            // Both jumping and falling far.
            LocalPlay {
                move_flags: move_flags::FORWARD | move_flags::FALLING | move_flags::FALLING_FAR,
                ..base
            },
            // A jump still rising.
            LocalPlay {
                move_flags: move_flags::FORWARD | move_flags::FALLING,
                vertical_speed: 3.0,
                ..base
            },
            // Turning while moving is a turn shuffle.
            LocalPlay {
                move_flags: move_flags::FORWARD | move_flags::TURN_RIGHT,
                ..base
            },
        ];
        for (n, i) in refused.iter().enumerate() {
            assert!(!local_play_eligible(i), "case {n}");
        }
        // A jump at its apex, and a turn while swimming or on a flying spline, pass.
        for i in [
            LocalPlay {
                move_flags: move_flags::FORWARD | move_flags::FALLING,
                ..base
            },
            LocalPlay {
                move_flags: move_flags::FORWARD | move_flags::TURN_RIGHT | move_flags::SWIMMING,
                ..base
            },
            LocalPlay {
                move_flags: move_flags::FORWARD | move_flags::TURN_RIGHT,
                flying: true,
                ..base
            },
        ] {
            assert!(local_play_eligible(&i));
        }
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
