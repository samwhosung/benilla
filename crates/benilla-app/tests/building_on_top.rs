//! The seam `docs/CONTRIBUTING.md` ("Building on top") opens to a crate: read the streamed objects
//! through [`benilla_app::Objects`] and interact with one through [`benilla_app::Interact`], the
//! chat stream through [`benilla_app::ChatEvent`], and the active dialogue text through
//! [`benilla_app::GossipState`] and [`benilla_app::QuestGiver`], from outside the crate. It fails
//! to build if any goes private, with no feature needed.

use benilla_app::{
    ChatEvent, ChatEventKind, GossipOption, GossipState, Interact, Objects, QuestGiver,
};
use bevy::prelude::*;

/// What such a crate writes: an object it found, interacted with.
fn interact_with_the_nearest(objects: Objects, mut interact: MessageWriter<Interact>) {
    if let Some((_, entity, _)) = objects.iter().next() {
        interact.write(Interact(entity));
    }
}

/// A key press such a crate answers: the guid it picked, interacted with.
fn interact_with_guid(guid: u64, objects: &Objects, interact: &mut MessageWriter<Interact>) {
    if let Some(entity) = objects.entity(guid) {
        interact.write(Interact(entity));
    }
}

/// The chat stream such a crate observes.
fn read_chat_stream(mut chat: MessageReader<ChatEvent>) {
    for event in chat.read() {
        let _ = (event.kind, &event.text, &event.sender);
    }
}

/// The active gossip dialogue such a crate observes.
fn read_gossip_dialogue(gossip: Res<GossipState>) {
    let _text: &str = gossip.text();
    let _options: &[GossipOption] = &gossip.options;
}

/// The active quest dialogue such a crate observes.
fn read_quest_dialogue(quest: Res<QuestGiver>) {
    let _state = quest.state();
}

fn is_a_system<M>(_: impl IntoSystem<(), (), M>) {}

#[test]
fn a_crate_on_top_reads_objects_and_writes_interact() {
    is_a_system(interact_with_the_nearest);
    let _ = interact_with_guid;

    let mut app = App::new();
    app.add_message::<Interact>();
    let entity = app.world_mut().spawn_empty().id();
    app.world_mut().write_message(Interact(entity));
    let sent: Vec<Interact> = app
        .world()
        .resource::<Messages<Interact>>()
        .iter_current_update_messages()
        .copied()
        .collect();
    assert_eq!(sent, [Interact(entity)]);
}

#[test]
fn a_crate_on_top_reads_chat_stream() {
    is_a_system(read_chat_stream);

    let mut app = App::new();
    app.add_message::<ChatEvent>();
    let event = ChatEvent {
        kind: Some(ChatEventKind::Say),
        text: "hello".into(),
        sender: "Someone".into(),
        ..Default::default()
    };
    app.world_mut().write_message(event.clone());
    let sent: Vec<ChatEvent> = app
        .world()
        .resource::<Messages<ChatEvent>>()
        .iter_current_update_messages()
        .cloned()
        .collect();
    assert_eq!(sent, [event]);
}

#[test]
fn a_crate_on_top_reads_gossip_and_quest_state() {
    is_a_system(read_gossip_dialogue);
    is_a_system(read_quest_dialogue);

    let mut app = App::new();
    app.init_resource::<GossipState>();
    app.init_resource::<QuestGiver>();

    let gossip = app.world().resource::<GossipState>();
    assert_eq!(gossip.text(), "");
    assert!(gossip.options.is_empty());

    let quest = app.world().resource::<QuestGiver>();
    assert_eq!(quest.state().title, "");
}
