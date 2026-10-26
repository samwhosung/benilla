//! The gossip window's packet handlers: they fill [`GossipState`], on the latch, signal the drawn
//! greeting on the gossip lane ([`super::lane`]), and touch the VM only through the close core.

use benilla_protocol::messages::{GossipOption, NpcTextBlock};
use benilla_protocol::{SessionEvent, SessionEventKind};
use bevy::prelude::*;

use super::{GossipEvent, GossipLane, GossipState};
use crate::names::NameCache;
use crate::net::{
    ClientCommand, Guid, GuidIndex, NetCommands, NetHandlerApp, ObjectStore, SelfPlayer,
};

/// Register the gossip handlers and the session-end listener.
pub(super) fn register(app: &mut App) {
    use SessionEventKind as K;
    app.net_handler(K::GossipMenu, on_gossip_menu)
        .net_handler(K::NpcGreeting, on_npc_greeting)
        .net_handler(K::GossipComplete, on_gossip_complete)
        .net_handler(K::Disconnected, on_session_end);
}

fn on_gossip_menu(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::GossipMenu {
        npc,
        text_id,
        options,
        quests,
    } = ev
    {
        commands.queue(move |world: &mut World| {
            gossip_menu(world, npc, text_id, options, quests);
        });
    }
}

fn on_npc_greeting(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::NpcGreeting { text_id, blocks } = ev {
        commands.queue(move |world: &mut World| npc_greeting(world, text_id, blocks));
    }
}

fn on_gossip_complete(In(ev): In<SessionEvent>, mut commands: Commands) {
    if let SessionEvent::GossipComplete = ev {
        crate::ui_session::queue(&mut commands, gossip_complete);
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
fn npc_gender(world: &World, guid: u64) -> u8 {
    world
        .get_resource::<GuidIndex>()
        .and_then(|index| index.0.get(&guid).copied())
        .and_then(|e| world.get::<ObjectStore>(e))
        .and_then(|s| s.0.unit_gender())
        .unwrap_or(0)
}

/// `SMSG_GOSSIP_MESSAGE` (`0x4e26e0`): the open `0x4e1fa0(npc)` registers on the latch unless the
/// cell already holds this NPC (`0x4e1fb8`), then writes the cell (`0x4e1ff0`); the menu is
/// latched, and on a first visit to its text the query goes out. The greeting is drawn at menu
/// open, as the reference's `0x4e2010` draws it; a record with no line closes the menu.
pub(crate) fn gossip_menu(
    world: &mut World,
    npc: u64,
    text_id: u32,
    options: Vec<GossipOption>,
    quests: Vec<(u32, u32, u32, String)>,
) {
    debug!(
        "net: gossip menu on {npc:#x} — {} options, {} quests",
        options.len(),
        quests.len()
    );
    if world.resource::<GossipState>().npc != Some(npc) {
        crate::ui_session::register(world, npc);
    }
    let gender = npc_gender(world, npc);
    let query = world
        .resource_mut::<GossipState>()
        .open_menu(npc, text_id, options, quests, gender);
    if query {
        if let Some(net) = world.get_resource::<NetCommands>() {
            let _ = net
                .0
                .send(ClientCommand::NpcTextQuery { text_id, guid: npc });
        }
    }
    show_drawn(world);
    close_on_missing_text(world);
}

/// `SMSG_NPC_TEXT_UPDATE`: cache the record and open the menu waiting on it, whose NPC's gender
/// picks the column.
fn npc_greeting(world: &mut World, text_id: u32, blocks: Vec<NpcTextBlock>) {
    let gender = world
        .resource::<GossipState>()
        .npc
        .map_or(0, |npc| npc_gender(world, npc));
    world
        .resource_mut::<GossipState>()
        .text_arrived(text_id, blocks, gender);
    show_drawn(world);
    close_on_missing_text(world);
}

/// The draw's tail in `0x4e2010`, once the record is found: with no active player or no object
/// under the cell `[0xbc3f58]` it bails (`0x4e20b3`, `0x4e20bb` → `0x4e22ba`), so a close's zeroed
/// cell writes no text and signals nothing, nor takes the missing-text close. Else the greeting
/// expanded for the player (`0x4e2274`), written to the text buffer (`0x4e229a`), then
/// `GOSSIP_SHOW` (`0x4e22b5`) on the gossip lane, with the NPC's name asked as its listener's
/// `UnitName("npc")` asks it there (`GossipFrame.lua:33`, `0x5171b8`).
fn show_drawn(world: &mut World) {
    let player = world
        .query_filtered::<(), (With<SelfPlayer>, With<ObjectStore>)>()
        .iter(world)
        .next()
        .is_some();
    let npc = world.resource::<GossipState>().npc;
    let held = |guid: u64| {
        world
            .get_resource::<GuidIndex>()
            .and_then(|index| index.0.get(&guid).copied())
            .is_some_and(|e| world.get::<ObjectStore>(e).is_some())
    };
    let Some(npc) = npc.filter(|&g| player && held(g)) else {
        let mut state = world.resource_mut::<GossipState>();
        state.take_drawn();
        state.take_missing_text();
        return;
    };
    let Some(raw) = world.resource_mut::<GossipState>().take_drawn() else {
        return;
    };
    let text = world
        .run_system_cached_with(expand_greeting, raw.clone())
        .unwrap_or(raw);
    world.resource_mut::<GossipState>().text = text;
    if let Some(mut lane) = world.get_resource_mut::<GossipLane>() {
        lane.push(GossipEvent::Show);
    }
    let _ = world.run_system_cached_with(ask_npc_name, npc);
}

/// The NPC's name looked up with a callback, as `UnitName("npc")` does at the show; its landing
/// fires nothing.
fn ask_npc_name(
    In(npc): In<u64>,
    names: Res<NameCache>,
    commands: Res<NetCommands>,
    index: Res<GuidIndex>,
    stores: Query<&ObjectStore>,
) {
    let store = index.0.get(&npc).and_then(|&e| stores.get(e).ok());
    let _ = names.resolve_unit(npc, store, crate::query_cache::Ask::Event(&commands));
}

/// The greeting's `$N`/`$B`/`$G`/`$<n>w` macros for the active player (`0x506f70`).
fn expand_greeting(
    In(raw): In<String>,
    self_q: Query<(&ObjectStore, &Guid), With<SelfPlayer>>,
    names: Res<NameCache>,
    unit_names: crate::ui_unit::NameTables,
    states: Res<crate::world_state::WorldStates>,
) -> String {
    let player = crate::npc_text::player_identity(&self_q, &names, unit_names.get());
    crate::npc_text::substitute(
        &raw,
        &crate::npc_text::MacroContext {
            subject: player.as_ref(),
            states: &states,
        },
    )
}

/// The missing-text path's close (`0x4e2197` → `0x4e1fa0(0)`).
fn close_on_missing_text(world: &mut World) {
    if world.resource_mut::<GossipState>().take_missing_text() {
        super::close_core(world);
    }
}

/// `SMSG_GOSSIP_COMPLETE` (`0x4e2800`): the menu's close core with a zero guid (`0x4e2824`), and
/// nothing else. A quest panel opened from the menu closed the menu when it took the latch, so the
/// panel stays.
fn gossip_complete(world: &mut World) {
    debug!("net: gossip complete — closing the menu");
    super::close_core(world);
}
