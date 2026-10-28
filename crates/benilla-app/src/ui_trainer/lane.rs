//! The trainer lane: the list's and the state passes' signals, each fired at its packet's place
//! among the other lanes' items ([`crate::lanes`]). In the reference each signals inside its
//! caller: the list builder its `TRAINER_SHOW` (`0x4d7b73`) after the per-list reset (`0x4d7560`)
//! and the sort (`0x4d7b3b`), the state pass `0x4d7d40` its `TRAINER_UPDATE` (`0x4d83f9`) after
//! the re-derive and the sort (`0x4d83ef`), run by each of its twelve callers. A handler queues its
//! item here, stamped where it runs; the firing pushes the window's services to the VM and sorts
//! its rows, and the getters' per-call reads go through the host ([`super::live`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use super::{TrainerOpen, TrainerReads};
use crate::lanes::{LaneApp, Stamp, Until};
use crate::names::NameCache;
use crate::net::NetCommands;
use crate::query_cache::Ask;

/// What an item signals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrainerEvent {
    /// A list's reset, push and sort, then `TRAINER_SHOW`.
    Show,
    /// One state pass, its push and sort, then `TRAINER_UPDATE`.
    Update,
    /// A close's clear of the VM's rows (`0x4d7481`, `0x4d7486`), behind the signals it parked.
    Cleared,
    /// What a close core run outside a call signalled while this lane held items, parked in the
    /// VM of the session under the key ([`crate::script_calls::park_since`]).
    Parked { session: u64, key: u64 },
}

/// The trainer lane's queue.
#[derive(Resource, Default)]
pub(crate) struct TrainerLane {
    events: VecDeque<(Stamp, TrainerEvent)>,
}

impl TrainerLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: TrainerEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    fn pop(&mut self, until: Until) -> Option<(Stamp, TrainerEvent)> {
        self.head().filter(|&s| until.admits(s))?;
        self.events.pop_front()
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, TrainerEvent)> {
        self.events.iter().copied().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_trainer_lane(app: &mut App) {
    app.init_resource::<TrainerLane>()
        .add_lane(
            "trainer",
            |world| world.get_resource::<TrainerLane>()?.head(),
            fire_trainer,
            None,
        )
        .add_systems(
            Update,
            drop_trainer_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_trainer_events(mut lane: ResMut<TrainerLane>) {
    lane.events.clear();
}

/// The VM and the host, with what the firing pushes from and the window it rewrites.
type TrainerVm<'w, 's> = crate::script_calls::VmWith<
    'w,
    's,
    (
        TrainerReads<'static, 'static>,
        ResMut<'static, TrainerOpen>,
        ResMut<'static, TrainerLane>,
        Title<'static, 'static>,
    ),
>;

/// The name the stock window titles with, `UnitName("npc")` (`Blizzard_TrainerUI.lua:90`), which
/// looks the trainer up with a callback (`0x5171b8`) at its events; its landing fires nothing.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Title<'w, 's> {
    names: Option<Res<'w, NameCache>>,
    commands: Option<Res<'w, NetCommands>>,
    index: Option<Res<'w, crate::net::GuidIndex>>,
    stores: Query<'w, 's, &'static crate::net::ObjectStore>,
}

impl Title<'_, '_> {
    fn ask(&self, trainer: u64) {
        let (Some(names), Some(commands)) = (self.names.as_deref(), self.commands.as_deref())
        else {
            return;
        };
        let store = self
            .index
            .as_deref()
            .and_then(|i| i.0.get(&trainer))
            .and_then(|&e| self.stores.get(e).ok());
        let _ = names.resolve_unit(trainer, store, Ask::Event(commands));
    }
}

/// The lane's firing: each item the drain admits, its push an engine turn and its signal with the
/// host lent. It stops after an item whose firing stamped anything, so what that pushed fires next.
fn fire_trainer(In(until): In<Until>, mut vm: TrainerVm) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    while let Some((at, event)) = vm.own().2.pop(until) {
        let ((), moved) = crate::lanes::firing(at, || fire(&mut vm, event));
        if moved {
            return;
        }
    }
}

/// One item: a parked close's signals, or a push and its event.
fn fire(vm: &mut TrainerVm, event: TrainerEvent) {
    if let TrainerEvent::Parked { session, key } = event {
        vm.run(|script| {
            if let Some(script) = script.filter(|s| s.session() == session) {
                crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Trainer, || {
                    script.run_parked(key)
                });
            }
        });
        return;
    }
    if event == TrainerEvent::Cleared {
        vm.run(|script| {
            if let Some(script) = script {
                script.set_trainer(None);
            }
        });
        return;
    }
    let signal = vm.turn(|script, (reads, mut open, _, title)| {
        push(script?, &reads, &mut open, &title, event)
    });
    if let Some(signal) = signal {
        vm.run(|script| {
            if let Some(script) = script {
                script.fire_event(signal, vec![]);
            }
        });
    }
}

/// A list's or a pass's work on the window and the VM before its signal, which it names: its
/// handler found the window open, and a close since parked behind it; nothing before the catalogs
/// load.
fn push(
    script: &mut UiScript,
    reads: &TrainerReads,
    open: &mut TrainerOpen,
    title: &Title,
    event: TrainerEvent,
) -> Option<&'static str> {
    let trainer = open.listed?;
    let signal = match event {
        TrainerEvent::Show => "TRAINER_SHOW",
        // `0x4d7d40` over every row, then the sort and `TRAINER_UPDATE` unconditionally, as its
        // tail fires it (`0x4d83f4`); the stock window re-reads `GetMoney()` on it.
        TrainerEvent::Update => {
            if !reads.pass(open) {
                return None;
            }
            "TRAINER_UPDATE"
        }
        TrainerEvent::Parked { .. } | TrainerEvent::Cleared => return None,
    };
    let state = {
        let lua = script.lua();
        reads.snapshot(open, benilla_ui::strings::Strings::Vm(lua))?
    };
    // The per-list reset goes in before the push, so `TRAINER_SHOW` finds it; the stock window
    // re-applies its saved filter only on its own `ADDON_LOADED`.
    if event == TrainerEvent::Show {
        script.reset_trainer_list_state(open.trainer_type);
    }
    script.set_trainer(state);
    title.ask(trainer);
    Some(signal)
}

/// Run the close `core` behind the lane ([`crate::lanes::behind`]), so a close never fires ahead
/// of the open it closes.
pub(super) fn behind_lane(
    world: &mut World,
    core: impl FnOnce(&mut World),
) -> crate::lanes::Behind {
    let head = world
        .get_resource::<TrainerLane>()
        .and_then(TrainerLane::head);
    crate::lanes::behind(world, head, core, |world, session, key| {
        world
            .resource_mut::<TrainerLane>()
            .push(TrainerEvent::Parked { session, key });
    })
}
