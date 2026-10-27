//! The dialogs' lane: the events and lines the dialog packets' handlers raise, each fired at its
//! packet's place among the other lanes' items ([`crate::lanes`]). In the reference each handler
//! writes its globals and signals inside itself: the innkeeper's `CONFIRM_BINDER` (`0x5dfdc0`), the
//! pet trainer's `CONFIRM_PET_UNLEARN` (`0x5dfba0`), the boot countdown's pair (`0x496340`), the
//! meeting stone's line and `MEETINGSTONE_CHANGED` (`0x4ca230`), the summon's `CONFIRM_SUMMON`
//! (`0x4963ce`), the lockout lines (`0x49e1c0`), the duel's events and lines (`0x4d49d0`-`0x4d4ba0`)
//! and the ticket's answers. A handler queues its signal here, stamped where it runs, and the
//! getters its listeners call read the live state through the host ([`super::live`]).

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::{ScriptValue, UiScript};

use crate::area::AreaTableRes;
use crate::lanes::{LaneApp, Stamp, Until};
use crate::ui_action::MessageSink;

/// What an item signals.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DialogSignal {
    /// An event and its arguments (`0x703e50`, `0x703f50`), queued by [`DialogLane::event`].
    Event(String, Vec<ScriptValue>),
    /// A catalog message by its key (`0x496720`).
    Line(&'static str),
    /// A catalog message over `%s` fills (`0x496720`).
    LineS(&'static str, Vec<String>),
    /// `CONFIRM_BINDER` named by the player's area at the packet (`0x5dfe5e`): the leaf's name,
    /// else its zone's, else `HOME_INN`.
    ConfirmBinder { leaf: Option<u32> },
    /// The meeting stone's `0x295` line for one arrival, the old and the new area by id
    /// (`0x4ca3a4`).
    StoneLine { old: u32, new: u32, status: u8 },
    /// A duel line the client composes, a system chat line (`0x4d4982`, `0x4d4c2a`).
    Duel(crate::ui_duel::OwedLine),
    /// A lockout line with its map's name, handed to the chat composer (`0x49a870`).
    Lockout(crate::ui_instance::LockoutLine, Option<String>),
}

/// The dialogs' lane's queue.
#[derive(Resource, Default)]
pub(crate) struct DialogLane {
    signals: VecDeque<(Stamp, DialogSignal)>,
}

impl DialogLane {
    /// Queue `signal` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, signal: DialogSignal) {
        let stamp = crate::lanes::stamp();
        let at = self.signals.partition_point(|(s, _)| *s < stamp);
        self.signals.insert(at, (stamp, signal));
    }

    /// Queue `event` with `args`.
    pub(crate) fn event(&mut self, event: &str, args: Vec<ScriptValue>) {
        self.push(DialogSignal::Event(event.to_string(), args));
    }

    fn head(&self) -> Option<Stamp> {
        self.signals.front().map(|(s, _)| *s)
    }

    /// What the queued items signal, in order, taking them.
    #[cfg(test)]
    pub(crate) fn take(&mut self) -> Vec<DialogSignal> {
        self.signals.drain(..).map(|(_, s)| s).collect()
    }

    /// The queued items with their stamps, in order, taking them.
    #[cfg(test)]
    pub(crate) fn take_stamped(&mut self) -> Vec<(Stamp, DialogSignal)> {
        self.signals.drain(..).collect()
    }
}

/// The lane, which waits for the in-game interface, as the reference's exists before the world it
/// watches, and empties at the session's end. Every dialog plugin adds it; the first one counts.
pub(crate) fn add_dialog_lane(app: &mut App) {
    if app.world().contains_resource::<DialogLane>() {
        return;
    }
    app.init_resource::<DialogLane>()
        .add_lane(
            "dialogs",
            |world| world.get_resource::<DialogLane>()?.head(),
            fire_dialogs,
            None,
        )
        .add_systems(
            OnExit(crate::client_state::ClientState::InWorld),
            end_session_dialogs.in_set(crate::ui_script::SessionEnd::Clears),
        );
}

fn end_session_dialogs(mut lane: ResMut<DialogLane>) {
    lane.signals.clear();
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_dialogs(
    In(until): In<Until>,
    mut vm: crate::script_calls::HostedVm,
    mut lane: ResMut<DialogLane>,
    areas: Option<Res<AreaTableRes>>,
    mut sink: MessageSink,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    while let Some(at) = lane.head().filter(|&s| until.admits(s)) {
        let Some((_, signal)) = lane.signals.pop_front() else {
            return;
        };
        let ((), moved) = crate::lanes::firing(at, || {
            vm.run(|script| {
                if let Some(script) = script {
                    fire(script, &mut sink, areas.as_deref(), signal);
                }
            })
        });
        if moved {
            return;
        }
    }
}

/// One item's signal.
fn fire(
    script: &mut UiScript,
    sink: &mut MessageSink,
    areas: Option<&AreaTableRes>,
    signal: DialogSignal,
) {
    use crate::ui_action::{keyed_line, keyed_line_s, show_now, Shown};
    use benilla_ui::messages::MsgKind;
    let get = |key: &str| script.lua().globals().get::<String>(key).ok();
    let line = match signal {
        DialogSignal::Event(name, args) => return script.fire_event(&name, args),
        DialogSignal::ConfirmBinder { leaf } => {
            let name = crate::ui_binder::area_name_of(leaf, areas, &|key| {
                get(key).filter(|t| !t.is_empty())
            });
            return script.fire_event("CONFIRM_BINDER", vec![ScriptValue::Str(name)]);
        }
        DialogSignal::Line(key) => keyed_line(script, key),
        DialogSignal::LineS(key, args) => {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            keyed_line_s(script, key, &args)
        }
        DialogSignal::StoneLine { old, new, status } => {
            super::stone_line(script, areas, old, new, status)
        }
        // The three duel templates are engine-composed, not catalog rows: unkeyed, as a keyed
        // unknown row would turn the line red.
        DialogSignal::Duel(owed) => {
            crate::ui_duel::owed_text(&owed, &get).map(|text| Shown::unkeyed(MsgKind::Chat, text))
        }
        DialogSignal::Lockout(line, map) => crate::ui_instance::lockout_text(
            &line,
            map.as_deref(),
            benilla_ui::strings::Strings::Vm(script.lua()),
        )
        .map(|text| Shown::unkeyed(MsgKind::Chat, text)),
    };
    // Shown through `0x496720` or the chat composer `0x49a870`, inside the handler that adds it.
    if let Some(line) = line {
        show_now(script, sink, "ui_dialog_verbs", line);
    }
}
