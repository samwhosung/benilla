//! The mail lane: what the mailbox's packet handlers, the record callbacks of its getters and the
//! list, and the pending-mail step signal, each fired at its packet's place among the other lanes'
//! items ([`crate::lanes`]). In the reference each signals inside itself: the list parse's tail
//! (`0x4ad5c3`-`0x4ad5d6`), the result handler's arms and tail (`0x4ad050`, `0x4ad15f`), the
//! callbacks (`0x4ad010`, `0x4ae710`, `0x4af080`, `0x4af0a0`, `0x4af340`, `0x4af5b0`), the time
//! reply (`0x4ad605`), the arrival (`0x4ad66b`) and the world frame's step (`0x4adeba`). A handler
//! queues its item here, stamped where it runs, and the getters its listeners call read the
//! session through the host ([`super::live`]). An open or a close run while the lane holds an
//! earlier item, or a close at a packet, queues its VM half here too ([`step`]), so neither lands
//! ahead of what the lane still holds; a close's tail runs at once.

use std::collections::VecDeque;

use bevy::prelude::*;

use benilla_ui::script::UiScript;

use crate::lanes::{LaneApp, Stamp, Until};
use crate::net::{ClientCommand, NetCommands};
use crate::ui_action::MessageSink;

use super::{MailPending, MailResult};

/// What an item signals.
#[derive(Debug, PartialEq)]
pub(crate) enum MailEvent {
    /// The list parse's tail: the request lock clears (`0x4ad5d0`), then `MAIL_INBOX_UPDATE`
    /// (`0x4ad5d6`).
    List,
    /// One `SMSG_SEND_MAIL_RESULT`'s arm and tail ([`super::announce_results`]).
    Result(MailResult),
    /// A record callback's `MAIL_INBOX_UPDATE` for a found record (`0x4ad02a`, `0x4af08f`,
    /// `0x4af0af`, `0x4af34f`, `0x4af5bf`).
    InboxUpdate,
    /// The attachment's record callback's `MAIL_SEND_INFO_UPDATE` for a found record
    /// (`0x4ae71f`).
    SendInfoUpdate,
    /// `UPDATE_PENDING_MAIL` (`0x4ad605`, `0x4ad66b`, `0x4adeba`).
    Pending,
    /// The open core's compose reset with its events and `MAIL_SHOW` (`0x4acd38`, `0x4acd42`).
    Show,
    /// The close core's silent compose reset (`0x4acd55` → `0x4acdc0(0)`).
    Reset,
    /// The close core's `MAIL_CLOSED` (`0x4acda3`), the VM's copy of the cell cleared with it.
    Closed,
    /// The close core's tail (`0x4acda8`, `0x4acdb1`): after a read, or an arrival while open,
    /// `MSG_QUERY_NEXT_MAIL_TIME`, stamping "no mail".
    Refresh,
}

/// The mail lane's queue.
#[derive(Resource, Default)]
pub(crate) struct MailLane {
    events: VecDeque<(Stamp, MailEvent)>,
}

impl MailLane {
    /// Queue `event` stamped now, behind everything stamped before it.
    pub(crate) fn push(&mut self, event: MailEvent) {
        let stamp = crate::lanes::stamp();
        let at = self.events.partition_point(|(s, _)| *s < stamp);
        self.events.insert(at, (stamp, event));
    }

    fn head(&self) -> Option<Stamp> {
        self.events.front().map(|(s, _)| *s)
    }

    /// The queued items with their stamps, in order.
    #[cfg(test)]
    pub(crate) fn stamped(&self) -> Vec<(Stamp, &MailEvent)> {
        self.events.iter().map(|(s, e)| (*s, e)).collect()
    }
}

/// The lane, and its drop while no in-game interface is up to hear it: a signal is for the
/// interface that was up.
pub(super) fn add_mail_lane(app: &mut App) {
    app.init_resource::<MailLane>()
        .add_lane(
            "mail",
            |world| world.get_resource::<MailLane>()?.head(),
            fire_mail,
            None,
        )
        .add_systems(
            Update,
            drop_mail_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

/// The drop keeps a close's tail, whose query goes out whether or not an interface hears it.
fn drop_mail_events(
    mut lane: ResMut<MailLane>,
    mut pending: ResMut<MailPending>,
    commands: Option<Res<NetCommands>>,
) {
    for (_, event) in lane.events.drain(..) {
        if event == MailEvent::Refresh {
            refresh(&mut pending, commands.as_deref().map(|c| &c.0));
        }
    }
}

/// The close core's tail: the deferred refresh's one reader (`0x4acda8`), then the query and its
/// "no mail" stamp (`0x4acdb1` → `0x4adde0`).
fn refresh(pending: &mut MailPending, net: Option<&crossbeam_channel::Sender<ClientCommand>>) {
    if pending.take_refresh() {
        pending.on_query_sent();
        if let Some(net) = net {
            let _ = net.send(ClientCommand::QueryNextMailTime);
        }
    }
}

/// The lane's firing: each item the drain admits, signalled with the host lent. It stops after an
/// item whose firing stamped anything, so what that pushed fires next.
fn fire_mail(
    In(until): In<Until>,
    mut vm: crate::script_calls::VmWith<ResMut<'static, MailPending>>,
    mut lane: ResMut<MailLane>,
    commands: Res<NetCommands>,
    mut sink: MessageSink,
) {
    if !vm.run(|script| script.is_some()) {
        return;
    }
    while let Some(at) = lane.head().filter(|&s| until.admits(s)) {
        let Some((_, event)) = lane.events.pop_front() else {
            return;
        };
        let ((), moved) = crate::lanes::firing(at, || match event {
            MailEvent::Refresh => refresh(&mut vm.own(), Some(&commands.0)),
            event => vm.run(|script| {
                if let Some(script) = script {
                    fire(script, &mut sink, event);
                }
            }),
        });
        if moved {
            return;
        }
    }
}

/// One item's signal.
fn fire(script: &mut UiScript, sink: &mut MessageSink, event: MailEvent) {
    match event {
        MailEvent::List => super::announce_lists(script, 1),
        MailEvent::Result(result) => {
            super::announce_results(script, vec![result], |script, line| {
                if let Some(line) = line {
                    crate::ui_action::show_now(script, sink, "ui_mail", line);
                }
            });
        }
        MailEvent::InboxUpdate => script.fire_event("MAIL_INBOX_UPDATE", vec![]),
        MailEvent::SendInfoUpdate => script.fire_event("MAIL_SEND_INFO_UPDATE", vec![]),
        MailEvent::Pending => script.fire_event("UPDATE_PENDING_MAIL", vec![]),
        event @ (MailEvent::Reset | MailEvent::Closed) => {
            crate::ui_session::in_parked_close(crate::ui_session::NpcWindow::Mail, || {
                vm_step(script, &event)
            })
        }
        event => vm_step(script, &event),
    }
}

/// An open's or a close's VM half, at its caller or at its place on the lane.
fn vm_step(script: &mut UiScript, event: &MailEvent) {
    match event {
        // The open core (`0x4acd10`) resets the compose tab before `MAIL_SHOW`, the stationery
        // selection with it (`0x4ace07`), which `MAIL_SHOW`'s reset re-picks as row 1; the reset's
        // `MAIL_SEND_SUCCESS`, page-turn sound and all, is the reference's too.
        MailEvent::Show => {
            script.reset_compose_tab();
            script.fire_event("MAIL_SHOW", vec![]);
        }
        MailEvent::Reset => script.reset_compose_tab_silently(),
        MailEvent::Closed => {
            script.mail_closed();
            script.fire_event("MAIL_CLOSED", vec![]);
        }
        _ => {}
    }
}

/// Whether an open or a close starting now waits behind the lane ([`crate::lanes::waits_behind`]):
/// an earlier signal has not fired yet.
pub(super) fn waits(world: &World) -> bool {
    crate::lanes::waits_behind(world.get_resource::<MailLane>().and_then(MailLane::head))
}

/// One step of an open or a close: its VM half queued on the lane when it `waits` ([`waits`]), so
/// it runs after what the lane holds, a close's listeners re-entering it as the session it closed
/// ([`crate::ui_session::in_parked_close`]); else run now, its signals fired with the host lent.
/// A close's tail, the countdown's stamp and its query ([`MailEvent::Refresh`]), runs now either
/// way, inside the handler as the reference's does (`0x4acda8`), so a later packet's arrival
/// stamps over it. The reference reads the flag after `MAIL_CLOSED`'s listeners, so a
/// `MarkMailRead` among them (`0x4adda6`) is read in the same close; read ahead of them here, it
/// waits for the next close. Both orders hold once the lanes fire inside the dispatch.
pub(super) fn step(world: &mut World, waits: bool, event: MailEvent) {
    if waits && event != MailEvent::Refresh {
        if let Some(mut lane) = world.get_resource_mut::<MailLane>() {
            lane.push(event);
            return;
        }
    }
    match event {
        MailEvent::Refresh => {
            let net = world.get_resource::<NetCommands>().map(|n| n.0.clone());
            if let Some(mut pending) = world.get_resource_mut::<MailPending>() {
                refresh(&mut pending, net.as_ref());
            }
        }
        event => crate::ui_session::fire(world, |script| vm_step(script, &event)),
    }
}
