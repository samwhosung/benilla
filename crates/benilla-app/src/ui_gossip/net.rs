//! The gossip window's packet handlers (decision 0081 phase 3; in the net handler table since
//! 2318, moved out of the drain's npc arm file) — each fills the [`GossipState`] the gossip feed
//! ([`super`]) reads; nothing here touches the VM.

use benilla_protocol::messages::{GossipOption, NpcTextBlock};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::GossipState;
use crate::net::{ClientCommand, GuidIndex, NetCommands, NetHandlerApp, ObjectStore};
use crate::ui_quest::QuestGiver;

/// Register the gossip handlers — called from [`super::UiGossipPlugin`]. One per kind, plus the
/// session-end listener.
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

/// An open gossip menu dies with the socket. A listener on the session end
/// (a second handler on the kind, after the bridge's own teardown).
fn on_session_end(In(_): In<SessionEvent>, mut gossip: ResMut<GossipState>) {
    gossip.clear_session();
}

/// A streamed unit's gender (`UNIT_FIELD_BYTES_0` byte 2) by guid — the gossip greeting's column
/// selector (`0x4e20c1`: tested `== 1` for female, so genderless `2` reads as
/// male). `0` when the guid isn't streamed in or carries no descriptor yet, which is the same
/// column the reference takes for a gossip target that isn't a unit at all.
fn npc_gender(guid: u64, index: &GuidIndex, stores: &Query<&ObjectStore>) -> u8 {
    index
        .0
        .get(&guid)
        .and_then(|&e| stores.get(e).ok())
        .and_then(|s| s.0.unit_gender())
        .unwrap_or(0)
}

/// A gossip menu opened (`SMSG_GOSSIP_MESSAGE`): fill the [`GossipState`] the gossip feed
/// ([`super`]) reads. A first visit to the text id sends the ask-once
/// `CMSG_NPC_TEXT_QUERY` and **the feed fires nothing until [`npc_greeting`] answers it** — a
/// hidden frame stays hidden, an open one keeps its previous menu painted (B292's hold and
/// 1994's no-event edge; the mechanics and the reference law live on
/// [`GossipState::open_menu`]); a revisit serves from the cache and repaints right away.
/// [`gossip_complete`] closes it.
///
/// The greeting is **drawn here**, not at the packet — this is the reference's own moment for it
/// (`0x4e2010`), and the draw needs both this NPC's gender ([`npc_gender`]) and a fresh roll.
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

/// The NPC-text answer (`SMSG_NPC_TEXT_UPDATE`) — seed the cache with the whole record, and open
/// the menu still waiting on it (a late answer for a menu we already closed or switched just
/// seeds the cache; the next open draws its own line).
///
/// The record answers a query we sent for the waiting menu, so its NPC is the one whose gender
/// picks the column (decision 0081's ask-once flow).
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

/// `SMSG_GOSSIP_COMPLETE` ends the whole interaction (e.g. right after a quest accept), so the
/// quest window closes with the gossip menu.
pub(crate) fn gossip_complete(gossip: &mut GossipState, quest: &mut QuestGiver) {
    debug!("net: gossip complete — closing the menu");
    gossip.clear();
    quest.clear();
}
