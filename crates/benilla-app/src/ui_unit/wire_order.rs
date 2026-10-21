//! The wire order of the interface's two packet-fed event streams. The reference's handlers fire
//! their events inside the dispatch (`0x537aa0`), so the events come out in the order their
//! packets did. Here a handler queues what it fires: the field notices
//! ([`crate::field_watch::FieldNotice`]), fired by [`super::field_events::fire_field_notices`]
//! after the snapshots their handlers read, and the chat queue ([`crate::ui_chat::ChatLog`]: the
//! combat lines, `UNIT_COMBAT` and the combat edges), fired later in the frame by its drain. The
//! drain of the handler table marks both queues after each handler call, so an item's call is
//! known by its place in its queue, and a notice queued after a chat item waits for the chat drain,
//! which fires the two in that order. Within one call the notices come first, as they did before
//! the marks.

use bevy::ecs::message::Messages;
use bevy::prelude::*;

use crate::field_watch::FieldNotice;
use crate::ui_chat::ChatLog;

/// Both queues' counts at the start of this frame's drain and after each handler call it made.
#[derive(Resource, Default)]
pub(crate) struct WireMarks(Vec<Mark>);

#[derive(Clone, Copy, Debug)]
struct Mark {
    /// [`ChatLog::pushed`].
    chat: u64,
    /// The next field notice's message id.
    notice: usize,
}

impl WireMarks {
    /// The call that queued chat item `seq`: 0 before the drain, `k` in its `k`th handler call,
    /// one past the last after it.
    pub(crate) fn of_chat(&self, seq: u64) -> usize {
        self.0.partition_point(|m| m.chat <= seq)
    }

    /// The call that wrote field notice `id`, on the same scale as [`Self::of_chat`].
    pub(crate) fn of_notice(&self, id: usize) -> usize {
        self.0.partition_point(|m| m.notice <= id)
    }
}

/// Both queues' counts now, with the world holding both.
fn counts(world: &World) -> Option<Mark> {
    let chat = world.get_resource::<ChatLog>()?.pushed();
    let notices = world.get_resource::<Messages<FieldNotice>>()?;
    Some(Mark {
        chat,
        notice: notices.oldest_message_count() + notices.len(),
    })
}

/// Ahead of a drain: the last drain's marks are spent, and what is queued now came before it.
pub(crate) fn begin_drain(world: &mut World) {
    let start = counts(world);
    if let Some(mut marks) = world.get_resource_mut::<WireMarks>() {
        marks.0.clear();
        marks.0.extend(start);
    }
}

/// After one handler call: both queues' counts, which close the call.
pub(crate) fn mark_call(world: &mut World) {
    let now = counts(world);
    if let (Some(now), Some(mut marks)) = (now, world.get_resource_mut::<WireMarks>()) {
        marks.0.push(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chat item and a notice compare by the call that queued each; with no drain both read 0.
    #[test]
    fn an_item_reads_the_call_that_queued_it() {
        let mut world = World::new();
        world.init_resource::<ChatLog>();
        world.init_resource::<Messages<FieldNotice>>();
        world.init_resource::<WireMarks>();
        let log_edge = |world: &mut World| world.resource::<ChatLog>().push_combat_edge(true);
        let notice = |world: &mut World| {
            world
                .resource_mut::<Messages<FieldNotice>>()
                .write(FieldNotice::Global { id: 190 });
        };
        let marks = |world: &World| {
            let m = world.resource::<WireMarks>();
            (m.of_chat(0), m.of_notice(0), m.of_chat(1), m.of_notice(1))
        };
        assert_eq!(marks(&world), (0, 0, 0, 0), "no drain, no order");

        begin_drain(&mut world);
        log_edge(&mut world);
        mark_call(&mut world);
        notice(&mut world);
        mark_call(&mut world);
        // Before the next drain, after this one's calls.
        log_edge(&mut world);
        notice(&mut world);
        assert_eq!(
            marks(&world),
            (1, 2, 3, 3),
            "the edge in call 1, the notice in call 2, then both after the drain"
        );
    }
}
