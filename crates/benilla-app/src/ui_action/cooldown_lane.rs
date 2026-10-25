//! The cooldown lanes: each list replayed packet by packet, and each flush's events fired at its
//! place among the other lanes' items ([`crate::lanes`]). In the reference each cooldown packet's
//! handler writes its list and signals inside itself (`SMSG_SPELL_COOLDOWN` `0x6e9460` calls
//! `0x4b31b0` at `0x6e95b0`, and for the pet's list `0x4bce90` at `0x6e95b9`; `SMSG_ITEM_COOLDOWN`
//! `0x6e95d0` calls `0x4b31b0` and `0x4f93d0` at `0x6e9656` and `0x6e965b`; the event and the
//! clear call `0x6e3050`, which signals the pair, the usable refresh, then the bags or the pet's
//! flush at `0x6e3071`, `0x6e3076` and `0x6e3089` or `0x6e3080`; the cheat calls `0x6e9700`, the
//! pair, then the bags or the pet's flush), so its listeners read its own records through the
//! getters' queries (`0x6e2ea0`, `0x6e2ed0`; the pet bar's `0x6e2ea0` on list 1 at `0x4be03c`).
//! Each list logs its writes and its signals as the handlers make them
//! ([`crate::spell::Cooldowns`]); its lane rewinds it to the frame's start, replays each write at
//! its stamp, and fires each signal there with the host lent. The player's list is its resource,
//! the pet's [`crate::ui_pet::PetBar::cooldowns`].

use bevy::prelude::*;

use crate::lanes::{LaneApp, Until};
use crate::spell::{Cooldowns, Signal};
use crate::ui_pet::PetBar;

/// The resource that holds a cooldown list.
pub(crate) trait HoldsList: Resource {
    fn list_mut(&mut self) -> &mut Cooldowns;
}

impl HoldsList for Cooldowns {
    fn list_mut(&mut self) -> &mut Cooldowns {
        self
    }
}

impl HoldsList for PetBar {
    fn list_mut(&mut self) -> &mut Cooldowns {
        &mut self.cooldowns
    }
}

/// The two lanes, the player's ahead of the pet's at a tie, their ends and the drops under the
/// exact complement of the drain's gate.
pub(super) fn add_cooldown_lane(app: &mut App) {
    app.init_resource::<Cooldowns>()
        .add_lane(
            "cooldowns",
            |world| world.get_resource::<Cooldowns>()?.lane_head(),
            fire_cooldowns::<Cooldowns>,
            None,
        )
        .add_lane(
            "pet cooldowns",
            |world| world.get_resource::<PetBar>()?.cooldowns.lane_head(),
            fire_cooldowns::<PetBar>,
            None,
        )
        .add_drain_end(end_cooldowns::<Cooldowns>)
        .add_drain_end(end_cooldowns::<PetBar>)
        .add_systems(
            Update,
            (drop_cooldown_log::<Cooldowns>, drop_cooldown_log::<PetBar>)
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

/// A lane's firing: the list rewound to the frame's start, once per drain, at the origin and
/// alone, so every other lane's start is pushed before a signal fires; then each write the drain
/// admits replayed and each signal fired at its place with the host lent, the usable refresh
/// through the action states ([`crate::script_calls::VmWith::engine_usable_refresh`]). It stops
/// after a signal whose firing stamped anything, so what that pushed fires next.
fn fire_cooldowns<R: HoldsList>(
    In(until): In<Until>,
    mut vm: crate::script_calls::VmWith<ResMut<'static, R>>,
) {
    if vm.own().list_mut().rewind() {
        return;
    }
    loop {
        let Some((at, signal)) = vm.own().list_mut().replay(until) else {
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
            Signal::Pet => vm.run(|script| {
                if let Some(script) = script {
                    script.fire_event("PET_BAR_UPDATE_COOLDOWN", vec![]);
                }
            }),
        });
        if moved {
            return;
        }
    }
}

/// The drain's end, however it ended: the list as the frame left it.
fn end_cooldowns<R: HoldsList>(world: &mut World) {
    if let Some(mut holder) = world.get_resource_mut::<R>() {
        holder.list_mut().end_replay();
    }
}

/// What the packets wrote while no in-game interface is up, left unreplayed: no VM hears it.
fn drop_cooldown_log<R: HoldsList>(holder: Option<ResMut<R>>) {
    if let Some(mut holder) = holder {
        holder.list_mut().drop_log();
    }
}

/// The player's lane fired alone with no host, for a test that drives the list without the drain: each
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
    end_cooldowns::<Cooldowns>(world);
}
