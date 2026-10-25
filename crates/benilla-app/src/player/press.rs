//! A movement command's press or release, among the frame's input records and ahead of its packets:
//! every Lua movement function goes through `CInputControl::SetCommands` (`0x515090`), which sets
//! or clears the bit (`0x514840`, `0x514b70`), cancels the cast for a new direction bit
//! (`0x5150ce`-`0x515175`) and ends by running the movement tick inside the record
//! (`0x5151a0` → `0x514640`). The tick reads root and stun as they stand then (`0x514560`,
//! `0x5145b0`), so its start or stop is queued, and goes out on the wire, ahead of anything this
//! frame's packets apply. [`super::controller::control`] keeps the frame time: it runs the same
//! axes and flag build after the packets and finds the word this changed already streamed.

use bevy::prelude::*;

use crate::creature_anim::move_flags;
use crate::net::{Embodied, NetCommands};

use super::controller::{body_gates, loot_walk_away, BodyGates};
use super::{flags, input, move_trace, movement_net, state, swim, view_subject};
use super::{CameraControl, Player};

/// The frame's `SetCommands` calls ([`self`]): the autorun toggle and the cast cancel at every
/// press, then, when the held command bits changed, the tick on the body we drive.
#[allow(clippy::type_complexity)]
pub(super) fn press_tick(
    time: Res<Time>,
    binds: Res<crate::bindings::BindingsState>,
    rig: Res<CameraControl>,
    mut player: ResMut<Player>,
    net: Res<NetCommands>,
    gates: (
        Res<super::StandPending>,
        Res<view_subject::ViewSubject>,
        Query<Has<Embodied>, With<crate::net::SelfPlayer>>,
        Query<Option<&crate::net::ObjectStore>, With<Embodied>>,
        // The active mover `0xc4da98`, as whether it is our own character.
        Query<Has<crate::net::SelfPlayer>, With<crate::net::ActiveMover>>,
    ),
    // The cast bar's self-cancel and the loot window's move-start close.
    mut cancel: ResMut<crate::spell::LocalMoveStart>,
    mut loot: ResMut<crate::ui_loot::LootMoveStart>,
) {
    let word = input::command_word(&binds, &rig);
    let changed = std::mem::replace(&mut player.input_word, word) != word
        || binds.fired(crate::bindings::Input::ToggleAutoRun);
    if !player.active || player.detached {
        return;
    }
    let both_buttons = input::look_input(&binds, &player, &rig).both_buttons;
    // The cast bar's self-cancel, at the command press ([`input::cancels_cast`]), ahead of the
    // tick: between the press `0x5150a7` and the cancel `0x51512a` stands only the taint test
    // `0x494a50`, so it runs whoever drives the body. It reads the commands, never the streamed
    // word, which a root empties of direction bits. The cast cancel tests no mover (`0x5150f2`);
    // the channel's compares the player with the active mover (`0x51517b`-`0x515191`), which
    // possession sets to the creature and lost control zeroes (`0x5fa600`), so steering a possessed
    // creature, or pressing while feared, ends a cast but not a channel.
    let active_mover = gates.4.iter().next();
    let autorun_on = input::toggle_autorun(&binds, &mut player);
    if input::cancels_cast(
        &binds,
        autorun_on,
        input::both_engaged(&binds, &rig, both_buttons),
        active_mover.is_some(),
    ) {
        cancel.pressed = true;
        cancel.mover_is_player = active_mover == Some(true);
    }
    // `0x514840` and `0x514b70` answer whether a bit changed, and nothing else runs when none did.
    if !changed {
        return;
    }
    // The tick finds no unit while control is lost or the claimed mover is unstreamed
    // (`0x51466e`), and a server ride fails its precondition (`0x5144e0`): the frame time's
    // not-driving path parks and clears as before. A held or falling mover is the frame time's
    // too: the settle hold streams no locomotion, and in the air only the mover's standstill nudge
    // breaks the silence, which its step decides.
    if player.server_riding
        || player.control_lost
        || player.reseat
        || player.settling
        || player.move_flags & move_flags::FALLING != 0
    {
        return;
    }
    let BodyGates { stunned, mover, .. } = body_gates(
        gates.3.single().ok().flatten(),
        gates.2.single().unwrap_or(false),
        &gates.0,
        &player,
        &gates.1,
    );
    // `0x514560` and `0x5145b0` as the mover stands before this frame's packets.
    let may_translate = mover.may_translate(player.modes.rooted);
    let may_turn = mover.may_turn(stunned);
    let axes = input::move_axes(
        &binds,
        &mut player,
        &rig,
        both_buttons,
        may_translate,
        may_turn,
    );
    let swim = (player.move_flags & move_flags::SWIMMING != 0)
        .then(|| swim::translate_amounts(&axes, !may_translate));
    let bits =
        state::incapacitated_flags(flags::command_bits(&axes, swim), !may_translate, !may_turn);
    let prev = player.move_flags;
    let ended = movement_net::stream_commands(&net.0, &mut player, bits, time.elapsed_secs());
    player.press_ended_precast |= ended;
    move_trace::press(word, prev, player.move_flags);
    if player.foreign_mover.is_none() && loot_walk_away(prev, player.move_flags) {
        loot.0 = true;
    }
}
