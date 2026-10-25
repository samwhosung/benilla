//! The cooldown lane: the player's list replayed packet by packet, and each flush's events fired
//! at its place among the other lanes' items ([`crate::lanes`]). In the reference each cooldown
//! packet's handler writes the list and signals inside itself (`SMSG_SPELL_COOLDOWN` `0x6e9460`
//! calls `0x4b31b0` at `0x6e95b0`; `SMSG_ITEM_COOLDOWN` `0x6e95d0` calls `0x4b31b0` and `0x4f93d0`
//! at `0x6e9656` and `0x6e965b`; the event and the clear call `0x6e3050`, which signals the pair,
//! the usable refresh and the bags at `0x6e3071`, `0x6e3076` and `0x6e3089`; the cheat calls
//! `0x6e9700`), so its listeners read its own records through the getters' queries (`0x6e2ea0`,
//! `0x6e2ed0`). The list logs its writes and its signals as the handlers make them
//! ([`crate::spell::Cooldowns`]); the lane rewinds it to the frame's start, replays each write at
//! its stamp, and fires each signal there with the host lent.

use bevy::prelude::*;

use crate::lanes::{LaneApp, Until};
use crate::spell::{Cooldowns, Signal};

/// The lane, its end and the drop under the exact complement of the drain's gate.
pub(super) fn add_cooldown_lane(app: &mut App) {
    app.init_resource::<Cooldowns>()
        .add_lane(
            "cooldowns",
            |world| world.get_resource::<Cooldowns>()?.lane_head(),
            fire_cooldowns,
            None,
        )
        .add_drain_end(end_cooldowns)
        .add_systems(
            Update,
            drop_cooldown_log
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

/// The lane's firing: the list rewound to the frame's start, once per drain, at the origin and
/// alone, so every other lane's start is pushed before a signal fires; then each write the drain
/// admits replayed and each signal fired at its place with the host lent, the usable refresh
/// through the action states ([`crate::script_calls::VmWith::engine_usable_refresh`]). It stops
/// after a signal whose firing stamped anything, so what that pushed fires next.
fn fire_cooldowns(
    In(until): In<Until>,
    mut vm: crate::script_calls::VmWith<ResMut<'static, Cooldowns>>,
) {
    if vm.own().rewind() {
        return;
    }
    loop {
        let Some((at, signal)) = vm.own().replay(until) else {
            return;
        };
        let ((), moved) = crate::lanes::firing(at, || match signal {
            Signal::Pair => vm.run(|script| {
                if let Some(script) = script {
                    script.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
                    script.fire_event("SPELL_UPDATE_COOLDOWN", vec![]);
                }
            }),
            Signal::Usable => vm.engine_usable_refresh(),
            Signal::Bags => vm.run(|script| {
                if let Some(script) = script {
                    script.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
                }
            }),
        });
        if moved {
            return;
        }
    }
}

/// The drain's end, however it ended: the list as the frame left it.
fn end_cooldowns(world: &mut World) {
    if let Some(mut cooldowns) = world.get_resource_mut::<Cooldowns>() {
        cooldowns.end_replay();
    }
}

/// What the packets wrote while no in-game interface is up, left unreplayed: no VM hears it.
fn drop_cooldown_log(mut cooldowns: ResMut<Cooldowns>) {
    cooldowns.drop_log();
}

/// The lane fired alone with no host, for a test that drives the list without the drain: each
/// signal fired on the VM, the usable refresh run on the action states directly
/// ([`super::run_watcher_refresh_once`]), then the drain's end.
#[cfg(test)]
pub(crate) fn fire_cooldown_lane(world: &mut World) {
    let until = Until::all();
    while let Some((_, signal)) = world.resource_mut::<Cooldowns>().replay(until) {
        if signal == Signal::Usable {
            super::run_watcher_refresh_once(world);
            continue;
        }
        let Some(mut script) = world.get_non_send_resource_mut::<benilla_ui::script::UiScript>()
        else {
            continue;
        };
        if signal == Signal::Pair {
            script.fire_event("ACTIONBAR_UPDATE_COOLDOWN", vec![]);
            script.fire_event("SPELL_UPDATE_COOLDOWN", vec![]);
        } else {
            script.fire_event("BAG_UPDATE_COOLDOWN", vec![]);
        }
    }
    end_cooldowns(world);
}
