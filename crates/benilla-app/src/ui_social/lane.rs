//! The social lane: what the friend, ignore and `/who` handlers raise, each fired at its
//! packet's place among the other lanes' items ([`crate::lanes`]). In the reference each handler
//! signals inside itself: the sorts' `FRIENDLIST_UPDATE` (`0x5ae867`) and `IGNORELIST_UPDATE`
//! (`0x5ae92d`), `HandleResult`'s lines through `DisplayError` (`0x496720`), the name callbacks'
//! (`0x5ace70`, `0x5acf40`), and `SMSG_WHO`'s `WHO_LIST_UPDATE` or chat lines (`0x5adfca`). A
//! handler stamps what it raised here as it returns ([`SocialLane::take`]), each with the lists
//! it left, which the lane seats around its listeners ([`SocialState::swap_lists`]): the getters
//! (through the call host, [`super::live`]) and the index verbs read the list the signal left.

use std::collections::VecDeque;

use benilla_ui::script::{UiScript, WhoInfo};
use bevy::prelude::*;

use super::{feed, Heard, ListEvent, SocialState, Step};
use crate::lanes::{LaneApp, Stamp, Until};
use crate::script_calls::VmWith;
use crate::ui_action::{show_now, ui_error_text, MessageSink, Shown};

/// What the social handlers raised, each stamped as its handler returned.
#[derive(Resource, Default)]
pub(crate) struct SocialLane(VecDeque<(Stamp, Step)>);

impl SocialLane {
    /// Stamp what `social`'s handler raised, in its order, behind everything stamped before.
    pub(super) fn take(&mut self, social: &mut SocialState) {
        for step in social.heard.drain(..) {
            self.0.push_back((crate::lanes::stamp(), step));
        }
    }

    /// What waits, in order.
    #[cfg(test)]
    pub(super) fn heard(&self) -> impl DoubleEndedIterator<Item = &Heard> {
        self.0.iter().map(|(_, s)| &s.heard)
    }

    fn peek(&self) -> Option<Stamp> {
        self.0.front().map(|(s, _)| *s)
    }
}

/// The lane, and the drop of what no interface is up to hear: a new VM's first feed stands for
/// every list's signal ([`feed::feed_social`]).
pub(super) fn add_social_lane(app: &mut App) {
    app.init_resource::<SocialLane>()
        .add_lane(
            "social",
            |world| world.get_resource::<SocialLane>()?.peek(),
            fire_social,
            None,
        )
        .add_systems(
            Update,
            drop_social_events
                .in_set(crate::ui_script::UiFeed)
                .run_if(not(crate::ui_script::ingame_ui_up)),
        );
}

fn drop_social_events(mut lane: ResMut<SocialLane>) {
    lane.0.clear();
}

/// The lane's firing: each item `until` admits, with the host lent and its step's lists seated.
/// It stops after one whose firing stamped anything, so what that pushed fires next.
pub(super) fn fire_social(
    In(until): In<Until>,
    mut vm: VmWith<ResMut<'static, SocialState>>,
    mut lane: ResMut<SocialLane>,
    mut sink: MessageSink,
) {
    fire_items(until, &mut vm, &mut lane, &mut sink);
}

/// [`fire_social`]'s items, until one whose firing stamped anything: whether one did.
fn fire_items(
    until: Until,
    vm: &mut VmWith<ResMut<'static, SocialState>>,
    lane: &mut SocialLane,
    sink: &mut MessageSink,
) -> bool {
    if !vm.run(|script| script.is_some()) {
        return false;
    }
    while let Some(at) = lane.peek().filter(|&s| until.admits(s)) {
        let Some((_, Step { heard, mut lists })) = lane.0.pop_front() else {
            break;
        };
        vm.own().swap_lists(&mut lists);
        let ((), moved) = crate::lanes::firing(at, || {
            vm.run(|script| {
                if let Some(script) = script {
                    fire(script, sink, heard);
                }
            })
        });
        vm.own().swap_lists(&mut lists);
        if moved {
            return true;
        }
    }
    false
}

/// Every item waiting, fired alone, for a test that drives the lane without the drain.
#[cfg(test)]
pub(super) fn fire_all(
    mut vm: VmWith<ResMut<'static, SocialState>>,
    mut lane: ResMut<SocialLane>,
    mut sink: MessageSink,
) {
    while fire_items(Until::all(), &mut vm, &mut lane, &mut sink) {}
}

/// One thing a handler did that Lua hears.
fn fire(script: &mut UiScript, sink: &mut MessageSink, heard: Heard) {
    match heard {
        Heard::Event(ListEvent::Friends) => script.fire_event("FRIENDLIST_UPDATE", Vec::new()),
        Heard::Event(ListEvent::Ignores) => script.fire_event("IGNORELIST_UPDATE", Vec::new()),
        Heard::Line(line) => {
            let get = |key: &str| script.lua().globals().get::<String>(key).ok();
            let text = ui_error_text(&line, &get).unwrap_or_default();
            show_now(script, sink, "ui_social", Shown::keyed(line.key, text));
        }
        Heard::Who { raw, wire } => who(script, sink, raw, &wire),
    }
}

/// `SMSG_WHO`'s two exits (`0x5adfca`): `WHO_LIST_UPDATE`, or the records' chat lines in wire
/// order and the count line. `SetWhoToUI`'s flag (`0xc2a12c`) is read as the item fires, after
/// every listener stamped ahead of it, as the reference reads it in the handler (`0x5adf9c`).
fn who(script: &mut UiScript, sink: &mut MessageSink, raw: u32, wire: &[WhoInfo]) {
    if feed::answer_goes_to_the_frame(script.who_to_ui(), raw) {
        script.fire_event("WHO_LIST_UPDATE", Vec::new());
        return;
    }
    let printed = {
        let get = |key: &str| script.lua().globals().get::<String>(key).ok();
        feed::who_lines(wire, raw, &get)
    };
    crate::ui_action::show_messages(script, sink, "ui_social", printed);
}
