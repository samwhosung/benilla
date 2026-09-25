//! The gossip window's packet handlers: they fill [`GossipState`] and never touch the VM.

use benilla_protocol::messages::{GossipOption, NpcTextBlock};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::GossipState;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, ObjectStore};
use crate::ui_quest::QuestGiver;

/// Register the gossip handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::GossipMenu, on_gossip_menu)
        .net_handler(K::NpcGreeting, on_npc_greeting)
        .net_handler(K::GossipComplete, on_gossip_complete)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_gossip_menu(
    In(ev): In<SessionEvent>,
    mut gossip: ResMut<GossipState>,
    commands: Res<NetCommands>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    if let SessionEvent::GossipMenu {
        npc,
        text_id,
        options,
        quests,
    } = ev
    {
        gossip_menu(
            npc,
            text_id,
            options,
            quests,
            &mut gossip,
            &commands,
            &index,
            &stores,
        );
    }
}

fn on_npc_greeting(
    In(ev): In<SessionEvent>,
    mut gossip: ResMut<GossipState>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    if let SessionEvent::NpcGreeting { text_id, blocks } = ev {
        npc_greeting(text_id, blocks, &mut gossip, &index, &stores);
    }
}

fn on_gossip_complete(
    In(ev): In<SessionEvent>,
    mut gossip: ResMut<GossipState>,
    mut quest: ResMut<QuestGiver>,
) {
    if let SessionEvent::GossipComplete = ev {
        gossip_complete(&mut gossip, &mut quest);
    }
}

/// The menu closes with the connection.
fn on_session_end(In(_): In<SessionEvent>, mut gossip: ResMut<GossipState>) {
    gossip.clear_session();
}

/// A streamed unit's gender (`UNIT_FIELD_BYTES_0` byte 2), which picks the greeting's column: the
/// reference tests `== 1` for female (`0x4e20c1`), so genderless 2 reads male. `0` when the guid
/// is not a streamed unit; the reference's non-unit arm takes the female column only when block
/// 0 has no male text.
fn npc_gender(guid: u64, index: &GuidIndex, stores: &Query<&ObjectStore>) -> u8 {
    index
        .0
        .get(&guid)
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.unit_gender())
        .unwrap_or(0)
}

/// `SMSG_GOSSIP_MESSAGE`: latch the menu, and on a first visit to its text send the query. The
/// greeting is drawn at menu open, as the reference's `0x4e2010` draws it.
fn gossip_menu(
    npc: u64,
    text_id: u32,
    options: Vec<GossipOption>,
    quests: Vec<(u32, u32, u32, String)>,
    gossip: &mut GossipState,
    net_commands: &NetCommands,
    index: &GuidIndex,
    stores: &Query<&ObjectStore>,
) {
    let npc_gender = npc_gender(npc, index, stores);
    debug!(
        "net: gossip menu on {npc:#x} — {} options, {} quests",
        options.len(),
        quests.len()
    );
    if gossip.open_menu(npc, text_id, options, quests, npc_gender) {
        let _ = net_commands
            .0
            .send(ClientCommand::NpcTextQuery { text_id, guid: npc });
    }
}

/// `SMSG_NPC_TEXT_UPDATE`: cache the record and open the menu waiting on it, whose NPC's gender
/// picks the column.
fn npc_greeting(
    text_id: u32,
    blocks: Vec<NpcTextBlock>,
    gossip: &mut GossipState,
    index: &GuidIndex,
    stores: &Query<&ObjectStore>,
) {
    let npc_gender = gossip.npc.map_or(0, |npc| npc_gender(npc, index, stores));
    gossip.text_arrived(text_id, blocks, npc_gender);
}

/// `SMSG_GOSSIP_COMPLETE` ends the whole interaction, so the quest window closes with the menu.
pub(crate) fn gossip_complete(gossip: &mut GossipState, quest: &mut QuestGiver) {
    debug!("net: gossip complete — closing the menu");
    gossip.clear();
    quest.clear();
}
