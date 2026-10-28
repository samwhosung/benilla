//! The questgiver lane: what the panel packets' handlers and the item callbacks signal, each fired
//! at its packet's place among the other lanes' items ([`crate::lanes`]). In the reference each
//! signals inside itself: a panel's setter its event at its end (`0x500ee5`, `0x50105d`,
//! `0x501120`), the item cache's callback `QUEST_ITEM_UPDATE` (`0x501903`), the refusals and the
//! failures their lines through `DisplayError` (`0x5dbca0`, `0x5dc840`, `0x5e5ad0`) and the
//! turn-in its fanfare. A handler queues its item here, stamped where it runs, and the getters its
//! listeners call read the panel's cells through the host ([`super::live`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::{QuestPanel, ScriptValue, UiScript};

use crate::lanes::{LaneApp, Stamp, Until};
use crate::ui_action::{MessageSink, UiError};

/// What an item signals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QuestGiverEvent {
    /// A panel's event, for the giver the panel opened on.
    Panel(QuestPanel, u64),
    /// `QUEST_ITEM_UPDATE`, one per `GetQuestItemInfo` callback whose record landed (`0x5018c0`).
    ItemUpdate,
    /// A refusal's, a failure's, a share's or a quest chat line ([`super::lines`]).
    Line(UiError),
    /// The turn-in's `ERR_QUEST_REWARD_MONEY_S`, its coin string worded off the VM's globals
    /// (`0x5dc511`-`0x5dc609`).
    RewardMoney(u32),
    /// `QUEST_ACCEPT_CONFIRM` with the member's name and the quest's title (`0x5019fe`).
    AcceptConfirm { name: String, title: String },
    /// The turn-in's `QUESTCOMPLETED` fanfare.
    Fanfare,
    /// What a close core run outside a call signalled while this lane held items, parked in the
    /// VM of the session under the key ([`crate::script_calls::park_since`]).
    Parked { session: u64, key: u64 },
}

/// The questgiver lane's queue.
#[derive(Resource, Default)]
pub(crate) struct QuestGiverLane {
    events: VecDeque<(Stamp, QuestGiverEvent)>,
}

impl QuestGiverLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: QuestGiverEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    /// Queue the lines a handler raised on the window, stamped where it runs.
    pub(crate) fn push_lines(&mut self, giver: &mut super::QuestGiver) {
        for line in giver.take_messages() {
            self.push(QuestGiverEvent::Line(line));
        }
        if giver.take_completed_fanfare() {
            self.push(QuestGiverEvent::Fanfare);
        }
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    fn pop(&mut self, until: Until) -> Option<(Stamp, QuestGiverEvent)> {
        self.head().filter(|&s| until.admits(s))?;
        self.events.pop_front()
    }

    /// What the queued items signal, in order, taking them.
    #[cfg(test)]
    pub(crate) fn take(&mut self) -> Vec<QuestGiverEvent> {
        self.events.drain(..).map(|(_, e)| e).collect()
    }

    /// The queued lines, in order, taking every item.
    #[cfg(test)]
    pub(crate) fn take_lines(&mut self) -> Vec<UiError> {
        self.events
            .drain(..)
            .filter_map(|(_, e)| match e {
                QuestGiverEvent::Line(line) => Some(line),
                _ => None,
            })
            .collect()
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, QuestGiverEvent)> {
        self.events.iter().cloned().collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_quest_giver_lane(app: &mut App) {
    app.init_resource::<QuestGiverLane>()
        .add_lane(
            "quest giver",
            |world| world.get_resource::<QuestGiverLane>()?.head(),
            fire_quest_giver,
            None,
        )
        .add_systems(
            Update,
            drop_quest_giver_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_quest_giver_events(mut lane: ResMut<QuestGiverLane>) {
    lane.events.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_quest_giver(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut lane: ResMut<QuestGiverLane>,
    mut sink: MessageSink,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    while let Some((at, event)) = lane.pop(until) {
        let ((), moved) = crate::lanes::firing(at, || {
            vm.run(|script| {
                if let Some(script) = script {
                    fire(script, &mut sink, event);
                }
            })
        });
        if moved {
            return;
        }
    }
}

/// One item's signal: a panel's event carries no argument (`0x703e50`).
fn fire(script: &mut UiScript, sink: &mut MessageSink, event: QuestGiverEvent) {
    match event {
        QuestGiverEvent::Panel(panel, _) => script.fire_event(super::panel_event(panel), vec![]),
        QuestGiverEvent::ItemUpdate => script.fire_event("QUEST_ITEM_UPDATE", vec![]),
        QuestGiverEvent::Line(msg) => show(script, sink, msg),
        QuestGiverEvent::RewardMoney(copper) => {
            let get = |key: &str| script.lua().globals().get::<String>(key).ok();
            let coins = super::lines::coin_text(copper, &|key| get(key).unwrap_or_default());
            show(script, sink, UiError::s("ERR_QUEST_REWARD_MONEY_S", coins));
        }
        QuestGiverEvent::AcceptConfirm { name, title } => script.fire_event(
            "QUEST_ACCEPT_CONFIRM",
            vec![ScriptValue::Str(name), ScriptValue::Str(title)],
        ),
        QuestGiverEvent::Fanfare => script.queue_sound_kit("QUESTCOMPLETED"),
        QuestGiverEvent::Parked { session, key } => {
            if script.session() == session {
                crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Quest, || {
                    script.run_parked(key)
                });
            }
        }
    }
}

/// A line shown, and spoken, as its message row says, inside the handler that adds it.
fn show(script: &mut UiScript, sink: &mut MessageSink, msg: UiError) {
    let get = |key: &str| script.lua().globals().get::<String>(key).ok();
    if let Some(text) = crate::ui_action::ui_error_text(&msg, &get) {
        let shown = crate::ui_action::Shown::keyed(msg.key, text);
        crate::ui_action::show_now(script, sink, "ui_quest", shown);
    }
}

/// Run the close `core` behind the lane ([`crate::lanes::behind`]), so a close never fires ahead
/// of the panel it closes.
pub(super) fn behind_lane(world: &mut World, core: impl FnOnce(&mut World)) {
    let head = world
        .get_resource::<QuestGiverLane>()
        .and_then(QuestGiverLane::head);
    crate::lanes::behind(world, head, core, |world, session, key| {
        world
            .resource_mut::<QuestGiverLane>()
            .push(QuestGiverEvent::Parked { session, key });
    });
}
