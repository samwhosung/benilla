//! The guild-charter session: the registrar window, the petition window, and the verbs that
//! found a guild.
//!
//! The registrar is an NPC window on the latch: its `GUILD_REGISTRAR_SHOW` firer (`0x4f4fb0`,
//! firing at `0x4f5003`) registers through `CGGameUI::SetInteractNPC` (`0x4930d0`) like every NPC
//! window, closing whichever window holds it. The petition window is bound to a charter item with
//! no NPC, so walking away from the registrar leaves it open.
//!
//! `SMSG_PETITION_SHOW_SIGNATURES` carries no text: the name and requirement come from
//! `SMSG_PETITION_QUERY_RESPONSE`, the signer names from the name cache. It answers our request
//! and delivers an offer alike (`PetitionsHandler.cpp:390-397`); only `owner` tells them apart.

use benilla_protocol::messages::{
    petition_result, PetitionQueryResponse, PetitionRename, PetitionShowList,
    PetitionShowSignatures, PetitionSignResults,
};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::QueryCache;
use crate::ui_script::{SweepApp, SweepLeg, UiFeed, UiInput};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

mod feed;
mod lines;

/// The open registrar, apart from [`PetitionState`] so a close of the registrar leaves the charter.
#[derive(Resource, Default)]
pub(crate) struct GuildRegistrarState {
    open: Option<Registrar>,
}

#[derive(Clone, Debug)]
struct Registrar {
    /// What `BuyGuildCharter` addresses: the petition module's own latch (`[0xbdceb0]`).
    npc: u64,
    /// Price in copper, unsigned because the binding zero-extends it (`fild QWORD`).
    cost: u32,
}

/// The `UNIT_NPC_FLAGS` bits the 1.12 client requires, both of them, on a showlist's NPC
/// (`0x5eec33`, `0x5eec3b`); vmangos checks only `PETITIONER` before sending.
const REGISTRAR_NPC_FLAGS: u32 = crate::target::cursor_mode::npc_flags::PETITIONER
    | crate::target::cursor_mode::npc_flags::TABARDDESIGNER;

impl GuildRegistrarState {
    /// `SMSG_PETITION_SHOWLIST` (`0x5eeb80`) opens the registrar only if the NPC resolves as a
    /// unit (`0x5eec1a`) with both [`REGISTRAR_NPC_FLAGS`] and entry\[0\], never the first visible
    /// row, has `entryFlags & 1` (`0x5eec42`); anything else is dropped silently.
    fn passes(list: &PetitionShowList, npc_flags: Option<u32>) -> bool {
        if npc_flags.is_none_or(|f| f & REGISTRAR_NPC_FLAGS != REGISTRAR_NPC_FLAGS) {
            debug!(
                "ui_petition: showlist from {:#x} without both registrar npc flags — dropped",
                list.npc
            );
            return false;
        }
        if list.entries.first().is_none_or(|e| e.entry_flags & 1 == 0) {
            debug!("ui_petition: showlist entry[0] not flagged visible — dropped");
            return false;
        }
        true
    }

    /// Write the cell and the price for a showlist that [`Self::passes`]; `false` when it does
    /// not.
    fn open(&mut self, list: &PetitionShowList, npc_flags: Option<u32>) -> bool {
        if !Self::passes(list, npc_flags) {
            return false;
        }
        self.open = Some(Registrar {
            npc: list.npc,
            cost: list.entries[0].charter_cost as u32,
        });
        true
    }

    /// An open registrar on `npc`, past the gates [`Self::open`] runs, for a test.
    #[cfg(test)]
    pub(crate) fn open_for_test(&mut self, npc: u64) {
        self.open = Some(Registrar { npc, cost: 0 });
    }

    /// `GetGuildCharterCost()`: 0 with no registrar open.
    fn cost(&self) -> u32 {
        self.open.as_ref().map_or(0, |r| r.cost)
    }
}

impl GuildRegistrarState {
    /// The registrar's NPC, the cell `[0xbdceb0]`; `None` when it is closed.
    pub(crate) fn npc(&self) -> Option<u64> {
        self.open.as_ref().map(|r| r.npc)
    }

    /// The disconnect clear, which tells the VM nothing.
    fn clear(&mut self) {
        self.open = None;
    }
}

/// `CloseGuildRegistrar` (`0x4f5010`) is the core itself.
impl LatchedWindow for GuildRegistrarState {
    const WINDOW: NpcWindow = NpcWindow::Registrar;

    fn cell(&self) -> Option<u64> {
        self.npc()
    }

    fn dispatched(world: &mut World) {
        close_core(world);
    }

    fn take_close_verb(script: &mut benilla_ui::script::UiScript) -> bool {
        script.take_registrar_close()
    }
}

/// The close core `0x4f5010`: with a registrar open, the latch release (`0x4f5024`), the cell
/// cleared, then `GUILD_REGISTRAR_CLOSED` (`0x4f503a`).
pub(crate) fn close_core(world: &mut World) {
    let Some(npc) = world.resource::<GuildRegistrarState>().npc() else {
        return;
    };
    crate::ui_session::release(world, npc);
    world.resource_mut::<GuildRegistrarState>().clear();
    crate::ui_session::note_closed(world, NpcWindow::Registrar);
    crate::ui_session::fire(world, |script| {
        script.fire_event("GUILD_REGISTRAR_CLOSED", vec![]);
    });
}

/// `SMSG_PETITION_SHOWLIST` past its gates ([`GuildRegistrarState::open`]) opens the registrar
/// through `0x4f4fb0`: the latch first, with no test of the cell (`0x4f4fc0`), then the cell
/// (`0x4f4fdc`); the feed fires `GUILD_REGISTRAR_SHOW` (`0x4f5003`).
pub(crate) fn open_registrar(world: &mut World, list: &PetitionShowList, npc_flags: Option<u32>) {
    if !GuildRegistrarState::passes(list, npc_flags) {
        return;
    }
    crate::ui_session::register(world, list.npc);
    world
        .resource_mut::<GuildRegistrarState>()
        .open(list, npc_flags);
}

/// The open charter and the petition-record cache.
#[derive(Resource, Default)]
pub(crate) struct PetitionState {
    open: Option<OpenCharter>,
    /// The record cache by petition id, asked once through [`QueryCache`].
    records: QueryCache<u32, Record>,
    /// `[0xbdce1c]`: set while our sign is outstanding. A close's decline leg is gated on it
    /// (`0x4f3f89`), so closing while our own signature is in flight sends no decline.
    signing: bool,
    /// One step per landed or patched record. The charter tooltip's hover issues the record query,
    /// so `ui_items`' bag feed gates its rebuild on this or keeps the `None` it read first.
    records_epoch: u64,
    /// Lines composed at apply time, shown by the feed, which holds the `script` handle.
    lines: Vec<lines::Line>,
    /// What the feed last announced of the charter, across a `/reload`.
    fed: feed::FedPetition,
}

/// The charter the petition window is showing.
#[derive(Clone, Debug)]
struct OpenCharter {
    /// The charter item's guid, the handle every verb in the family takes.
    item: u64,
    /// The packet's owner; the originator's name and the close's decline test read it.
    owner: u64,
    petition_id: u32,
    /// In wire order; names resolve at feed time, so a late name repaints without a packet.
    signers: Vec<u64>,
}

/// One petition's record, from `SMSG_PETITION_QUERY_RESPONSE`.
#[derive(Clone, Debug, Default)]
struct Record {
    /// The record's owner (`+0x8`), not the packet's: `GetPetitionInfo`'s `isOriginator`
    /// compares against it (`0x4f447a`/`0x4f4481`), as does `CanSignPetition`'s owner refusal.
    owner: u64,
    /// The proposed guild's name.
    name: String,
    /// Free text, empty on vmangos.
    body_text: String,
    /// Signatures required, the record's `+0x1118`: signed, as the binding pushes it with
    /// `fild DWORD`. The Request-Signature button and `CanSignPetition` (`0x4f4634`) test it.
    required: i32,
    /// The record's `+0x1110` bit 0, set for a guild charter: it picks `GetPetitionInfo`'s first
    /// return and gates `CanSignPetition`'s guild-member and full-list refusals.
    is_charter: bool,
}

impl crate::query_cache::AskOnce for PetitionState {
    fn clear_pending(&mut self) {
        self.records.clear_pending();
    }
}

impl PetitionState {
    /// `SMSG_PETITION_SHOW_SIGNATURES`: open this charter and ask for its record once.
    fn show(&mut self, sigs: PetitionShowSignatures, commands: &NetCommands) {
        let petition_id = sigs.petition_id;
        self.open = Some(OpenCharter {
            item: sigs.item,
            owner: sigs.owner,
            petition_id,
            signers: sigs.signatures.into_iter().map(|s| s.signer).collect(),
        });
        self.request_record(petition_id, sigs.item, commands);
    }

    /// Ask once per petition id; the feed's next rebuild reads the answer.
    fn request_record(&self, petition_id: u32, item: u64, commands: &NetCommands) {
        self.records.get_or_ask(petition_id, || {
            let _ = commands
                .0
                .send(ClientCommand::PetitionQuery { petition_id, item });
        });
    }

    /// One step per landed or patched record.
    pub(crate) fn records_epoch(&self) -> u64 {
        self.records_epoch
    }

    /// `SMSG_PETITION_QUERY_RESPONSE`: fill the cache.
    fn apply_record(&mut self, response: PetitionQueryResponse) {
        self.records_epoch = self.records_epoch.wrapping_add(1);
        self.records.insert(
            response.petition_id,
            Some(Record {
                owner: response.owner,
                name: response.name,
                body_text: response.body_text,
                required: response.min_signatures as i32,
                is_charter: response.flags & 1 != 0,
            }),
        );
    }

    /// The charter tooltip's line 3, the guild name and its master: the hover asks for the record,
    /// so an unseen charter reads `None` until the answer repaints it.
    pub(crate) fn tooltip_view(
        &mut self,
        petition_id: u32,
        item: u64,
        names: &NameCache,
        commands: &NetCommands,
    ) -> Option<benilla_ui::script::PetitionSlotView> {
        if petition_id == 0 {
            return None;
        }
        self.request_record(petition_id, item, commands);
        let rec = self.records.get(petition_id)?;
        let (is_charter, title, owner_guid) = (rec.is_charter, rec.name.clone(), rec.owner);
        Some(benilla_ui::script::PetitionSlotView {
            is_charter,
            title,
            owner: names
                .resolve_player(owner_guid, commands)
                .map(str::to_string),
        })
    }

    /// The owner's half of a sign: append the signer, as the reference does (`0x4f41c0`), since no
    /// fresh list comes. Returns the name if cached, the condition for `ERR_PETITION_SIGNED_S`.
    fn append_signer(
        &mut self,
        item: u64,
        signer: u64,
        names: &NameCache,
        commands: &NetCommands,
    ) -> Option<String> {
        let open = self.open.as_mut().filter(|o| o.item == item)?;
        if !open.signers.contains(&signer) {
            open.signers.push(signer);
        }
        names.resolve_player(signer, commands).map(str::to_string)
    }

    /// The `MSG_PETITION_DECLINE` `ClosePetition()` sends (`0x4f3f60`'s leg at `0x4f3fb5`), only
    /// when a petition is open (`0x4f3f7c`), no sign of ours is in flight (`0x4f3f89`), a record
    /// is cached (`0x4f3f95`), and we are not the owner (`0x4f3fa5`/`0x4f3fae`). The reference
    /// reads that owner off the cached record (`0x4f3fa2`); this reads the packet's.
    fn decline_on_close(&mut self, me: u64, commands: &NetCommands) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        if self.signing || !self.records.answered(open.petition_id) || open.owner == me {
            return;
        }
        let _ = commands
            .0
            .send(ClientCommand::PetitionDecline { item: open.item });
    }

    /// The local half of `ClosePetition()`; the decline, if any, is `decline_on_close`.
    fn close(&mut self) {
        self.open = None;
    }

    /// The session end, with no decline. The record cache stays: it is keyed by the server's
    /// petition id, and the world-enter release re-arms its asks.
    fn clear_session(&mut self) {
        self.open = None;
        self.signing = false;
        self.lines.clear();
        self.forget_announced();
    }

    /// The UI session's end: the next session's VM is told the charter afresh.
    pub(crate) fn forget_announced(&mut self) {
        self.fed = feed::FedPetition::default();
    }

    /// The sweep's free of the signer list (`0x4f3eb0`): no signers, and none announced, as the
    /// free fires nothing.
    fn free_signers(&mut self) {
        if let Some(open) = self.open.as_mut() {
            open.signers.clear();
        }
        self.fed.forget_signers();
    }

    /// What `SignPetition`, `OfferPetition` and `RenamePetition` address.
    fn open_item(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.item)
    }
}

/// The two resources, the registrar on the latch, and the feed and drain.
pub(crate) struct UiPetitionPlugin;

impl Plugin for UiPetitionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::query_cache::register::<PetitionState>(app);
        app.init_resource::<GuildRegistrarState>()
            .init_resource::<PetitionState>()
            .add_latched_window::<GuildRegistrarState>()
            // `0x490b07` → `0x4f4fa0` → `0x4f5010`, the core.
            .add_sweep_leg(SweepLeg::Registrar, close_core)
            .add_sweep_leg(
                SweepLeg::Petition,
                feed::free_petition_signers_on_leave_world,
            )
            .add_systems(
                Update,
                (
                    feed::feed_petition.in_set(UiFeed),
                    feed::drain_petition.after(UiInput),
                ),
            );
    }
}

/// The inbound arms, one per packet. None fires an event or prints: lines queue on
/// [`PetitionState::lines`] for the feed, since a packet handler has no `script` handle.
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    use crate::net::{GuidIndex, ObjectStore, SelfGuid};
    use crate::ui_social::SocialState;

    pub(super) fn register(app: &mut App) {
        use SessionEventKind as K;
        app.net_handler(K::PetitionShowList, on_show_list)
            .net_handler(K::PetitionShowSignatures, on_show_signatures)
            .net_handler(K::PetitionQueryResponse, on_query_response)
            .net_handler(K::PetitionSignResults, on_sign_results)
            .net_handler(K::TurnInPetitionResults, on_turn_in_results)
            .net_handler(K::PetitionDeclined, on_declined)
            .net_handler(K::PetitionRenamed, on_renamed)
            .net_handler(K::Disconnected, on_session_end);
    }

    /// The charter and the registrar end with the session, or the next login shows the old charter
    /// and closing it sends a decline; the walk-away guard has no self player to measure from.
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        mut registrar: ResMut<GuildRegistrarState>,
    ) {
        petition.clear_session();
        registrar.clear();
    }

    /// The flags gate reads the live NPC; an unstreamed guid fails, as the client's resolve does.
    fn on_show_list(
        In(ev): In<SessionEvent>,
        index: Res<GuidIndex>,
        stores: Query<&ObjectStore>,
        mut commands: Commands,
    ) {
        if let SessionEvent::PetitionShowList(list) = ev {
            let flags = index
                .0
                .get(&list.npc)
                .and_then(|e| stores.get(*e).ok())
                .map(|s| s.0.unit_npc_flags());
            commands.queue(move |world: &mut World| super::open_registrar(world, &list, flags));
        }
    }

    /// The ignore list is consulted before anything else (`0x5eeefe`).
    fn on_show_signatures(
        In(ev): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        social: Res<SocialState>,
        commands: Res<NetCommands>,
    ) {
        if let SessionEvent::PetitionShowSignatures(sigs) = ev {
            let ignored = social.is_ignored(sigs.owner);
            show_signatures(&mut petition, sigs, ignored, &commands);
        }
    }

    fn on_query_response(In(ev): In<SessionEvent>, mut petition: ResMut<PetitionState>) {
        if let SessionEvent::PetitionQueryResponse(response) = ev {
            query_response(&mut petition, response);
        }
    }

    fn on_sign_results(
        In(ev): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        names: Res<NameCache>,
        self_guid: Res<SelfGuid>,
        commands: Res<NetCommands>,
    ) {
        if let SessionEvent::PetitionSignResults(results) = ev {
            sign_results(
                &mut petition,
                &names,
                self_guid.0.unwrap_or(0),
                results,
                &commands,
            );
        }
    }

    fn on_turn_in_results(
        In(ev): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        mut commands: Commands,
    ) {
        if let SessionEvent::TurnInPetitionResults { result } = ev {
            if turn_in_results(&mut petition, result) {
                commands.queue(|world: &mut World| {
                    crate::ui_session::fire(world, |script| {
                        script.fire_event("GUILD_REGISTRAR_CLOSED", vec![]);
                    });
                });
            }
        }
    }

    fn on_declined(
        In(ev): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        names: Res<NameCache>,
    ) {
        if let SessionEvent::PetitionDeclined { player } = ev {
            declined(&mut petition, &names, player);
        }
    }

    fn on_renamed(In(ev): In<SessionEvent>, mut petition: ResMut<PetitionState>) {
        if let SessionEvent::PetitionRenamed(rename) = ev {
            renamed(&mut petition, rename);
        }
    }

    /// `SMSG_PETITION_SHOW_SIGNATURES`: our charter or one offered to us. An ignored owner
    /// suppresses the whole update, silently (`0x5eeefe`-`0x5eef0b`).
    pub(crate) fn show_signatures(
        petition: &mut PetitionState,
        sigs: PetitionShowSignatures,
        owner_ignored: bool,
        commands: &NetCommands,
    ) {
        if owner_ignored {
            debug!(
                "ui_petition: charter from ignored owner {:#x} — whole update dropped",
                sigs.owner
            );
            return;
        }
        petition.show(sigs, commands);
    }

    /// `SMSG_PETITION_QUERY_RESPONSE`: the record cache fill.
    pub(crate) fn query_response(petition: &mut PetitionState, response: PetitionQueryResponse) {
        petition.apply_record(response);
    }

    /// `SMSG_PETITION_SIGN_RESULTS`: signer and owner get identical copies, told apart by the
    /// player guid (`0x5eefee`). Another's signature is appended, result unread (`0x4f41c0`); ours
    /// prints its line, clears the latch, and on success closes the window (`0x5ef037`).
    pub(crate) fn sign_results(
        petition: &mut PetitionState,
        names: &NameCache,
        self_guid: u64,
        results: PetitionSignResults,
        commands: &NetCommands,
    ) {
        if results.player != self_guid {
            let cached = petition.append_signer(results.item, results.player, names, commands);
            if let Some(name) = cached {
                petition.lines.push(lines::signed_by_other(&name));
            }
            return;
        }
        petition.signing = false;
        if let Some(line) = lines::my_sign_line(results.result) {
            petition.lines.push(line);
        }
        if results.result == petition_result::OK {
            petition.close();
        }
    }

    /// `SMSG_TURN_IN_PETITION_RESULTS`: success prints nothing and fires `GUILD_REGISTRAR_CLOSED`
    /// bare (`0x5ef166`), with the cell and the latch left for the frame's `OnHide`, whose
    /// `CloseGuildRegistrar()` runs the core; `true` when that event is owed. Only the two
    /// refusals print, both red.
    pub(crate) fn turn_in_results(petition: &mut PetitionState, result: u32) -> bool {
        if result == petition_result::OK {
            // The server destroyed the charter, so the item-bound window closes too.
            petition.close();
            return true;
        }
        if let Some(line) = lines::turn_in_line(result) {
            petition.lines.push(line);
        }
        false
    }

    /// `MSG_PETITION_DECLINE` inbound: a line only if the decliner's name is already cached, with
    /// no query or retry (`0x5ef12a`/`0x5ef139`), so this arm takes no command channel.
    pub(crate) fn declined(petition: &mut PetitionState, names: &NameCache, player: u64) {
        if let Some(name) = names.peek(player) {
            let line = lines::declined_line(name);
            petition.lines.push(line);
        }
    }

    /// `MSG_PETITION_RENAME` inbound: the echo's name overwrites the cached title in place, with
    /// no re-query (`0x5ef292`, which then fires `PETITION_SHOW`).
    pub(crate) fn renamed(petition: &mut PetitionState, rename: PetitionRename) {
        let Some(id) = petition
            .open
            .as_ref()
            .filter(|o| o.item == rename.item)
            .map(|o| o.petition_id)
        else {
            return;
        };
        if !petition.records.answered(id) {
            petition.records.insert(id, Some(Record::default()));
        }
        if let Some(record) = petition.records.get_mut(id) {
            record.name = rename.name;
        }
        // A patched title is a changed record: the tooltip's line 3 reads it too.
        petition.records_epoch = petition.records_epoch.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{PetitionShowListEntry, PetitionSignature};
    use crossbeam_channel::unbounded;

    fn commands() -> (NetCommands, crossbeam_channel::Receiver<ClientCommand>) {
        let (tx, rx) = unbounded();
        (NetCommands(tx), rx)
    }

    fn show_list(npc: u64, rows: &[(i32, i32)]) -> PetitionShowList {
        PetitionShowList {
            npc,
            entries: rows
                .iter()
                .enumerate()
                .map(|(i, (cost, flags))| PetitionShowListEntry {
                    index: i as u32 + 1,
                    charter_entry: benilla_protocol::messages::CHARTER_ITEM_ENTRY,
                    charter_display_id: benilla_protocol::messages::CHARTER_DISPLAY_ID,
                    charter_cost: *cost,
                    entry_flags: *flags,
                })
                .collect(),
        }
    }

    /// The open charter's cached title, `GetPetitionInfo`'s second return.
    fn open_title(p: &PetitionState) -> Option<&str> {
        let open = p.open.as_ref()?;
        p.records.get(open.petition_id).map(|r| r.name.as_str())
    }

    fn signatures(item: u64, owner: u64, id: u32, signers: &[u64]) -> PetitionShowSignatures {
        PetitionShowSignatures {
            item,
            owner,
            petition_id: id,
            signatures: signers
                .iter()
                .map(|s| PetitionSignature {
                    signer: *s,
                    unknown: 0,
                })
                .collect(),
        }
    }

    /// vmangos always sends one flagged row, so only a hidden first row tells entry\[0\] apart
    /// from the first visible row.
    #[test]
    fn the_registrar_gates_on_entry_zero_and_on_both_npc_flags() {
        let both = Some(REGISTRAR_NPC_FLAGS);

        let mut r = GuildRegistrarState::default();
        assert!(r.open(&show_list(0x2a1f, &[(1000, 1)]), both));
        assert_eq!(r.cost(), 1000, "entry[0]'s price, in copper");
        assert_eq!(r.npc(), Some(0x2a1f));

        // Entry[0] hidden: the whole packet is dropped, though row 1 is visible.
        let mut r = GuildRegistrarState::default();
        assert!(!r.open(&show_list(0x2a1f, &[(9999, 0), (1000, 1)]), both));
        assert_eq!(r.npc(), None, "no window at all — entry[0] is not visible");

        // Either flag alone fails; the client tests both.
        for flags in [
            None,
            Some(0),
            Some(crate::target::cursor_mode::npc_flags::PETITIONER),
            Some(crate::target::cursor_mode::npc_flags::TABARDDESIGNER),
        ] {
            let mut r = GuildRegistrarState::default();
            assert!(
                !r.open(&show_list(0x2a1f, &[(1000, 1)]), flags),
                "flags {flags:?} must not open the registrar"
            );
        }
    }

    /// The registrar's core, which the watchdog reaches through `0x493310`, clears only its own
    /// cell: the charter window is item-bound and stays.
    #[test]
    fn closing_the_registrar_leaves_an_open_charter_alone() {
        let (commands, _rx) = commands();
        let mut app = App::new();
        app.init_resource::<GuildRegistrarState>()
            .init_resource::<PetitionState>()
            .add_latched_window::<GuildRegistrarState>();
        let world = app.world_mut();
        open_registrar(
            world,
            &show_list(0x2a1f, &[(1000, 1)]),
            Some(REGISTRAR_NPC_FLAGS),
        );
        assert_eq!(
            world.resource::<crate::ui_session::InteractLatch>().guid(),
            Some(0x2a1f)
        );
        world
            .resource_mut::<PetitionState>()
            .show(signatures(0x99, 0xaa, 7, &[0xbb]), &commands);

        crate::ui_session::dispatch(world);
        assert_eq!(world.resource::<GuildRegistrarState>().npc(), None);
        assert_eq!(
            world.resource::<PetitionState>().open_item(),
            Some(0x99),
            "the charter window is item-bound and survives the walk-away"
        );
    }

    #[test]
    fn the_record_query_is_asked_once_per_petition() {
        let (commands, rx) = commands();
        let mut petition = PetitionState::default();
        petition.show(signatures(0x99, 0xaa, 7, &[]), &commands);
        petition.show(signatures(0x99, 0xaa, 7, &[0xbb]), &commands);
        let sent: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            sent.len(),
            1,
            "one query for two opens of the same petition"
        );
        assert!(matches!(
            sent[0],
            ClientCommand::PetitionQuery {
                petition_id: 7,
                item: 0x99
            }
        ));

        // The answer landing does not re-arm the ask.
        petition.apply_record(PetitionQueryResponse {
            petition_id: 7,
            name: "Legacy".into(),
            min_signatures: 9,
            ..Default::default()
        });
        petition.show(signatures(0x99, 0xaa, 7, &[0xbb, 0xcc]), &commands);
        assert!(rx.try_iter().next().is_none(), "still no second query");
        assert_eq!(open_title(&petition), Some("Legacy"));
    }

    #[test]
    fn a_rename_echo_patches_the_cached_record() {
        let (commands, _rx) = commands();
        let mut petition = PetitionState::default();
        petition.show(signatures(0x99, 0xaa, 7, &[]), &commands);
        petition.apply_record(PetitionQueryResponse {
            petition_id: 7,
            name: "First".into(),
            min_signatures: 9,
            ..Default::default()
        });
        net::renamed(
            &mut petition,
            PetitionRename {
                item: 0x99,
                name: "Second".into(),
            },
        );
        assert_eq!(open_title(&petition), Some("Second"));

        // An echo for a charter we do not have open is not ours to apply.
        net::renamed(
            &mut petition,
            PetitionRename {
                item: 0xdead,
                name: "Wrong".into(),
            },
        );
        assert_eq!(open_title(&petition), Some("Second"));
    }

    /// Through the real registration: a new login's VM never runs the old one's `OnHide`.
    #[test]
    fn the_session_end_closes_the_charter_and_forgets_the_sign() {
        let (commands, _rx) = commands();
        let mut app = App::new();
        app.add_plugins(UiPetitionPlugin);
        {
            let mut petition = app.world_mut().resource_mut::<PetitionState>();
            petition.show(signatures(0x99, 0xaa, 7, &[0xbb]), &commands);
            petition.signing = true;
        }
        app.world_mut()
            .resource_mut::<GuildRegistrarState>()
            .open(&show_list(0x2a1f, &[(1000, 1)]), Some(REGISTRAR_NPC_FLAGS));

        crate::net::handlers::dispatch(
            app.world_mut(),
            vec![benilla_protocol::SessionEvent::Disconnected {
                reason: "socket".into(),
                end: benilla_protocol::SessionEnd::Lost,
            }],
        );

        assert_eq!(app.world().resource::<GuildRegistrarState>().npc(), None);
        let petition = app.world().resource::<PetitionState>();
        assert_eq!(
            petition.open_item(),
            None,
            "no charter carried into the next login"
        );
        assert!(
            !petition.signing,
            "no sign of the old session still in flight"
        );
    }

    /// A `/reload` with a charter shown: every `PETITION_SHOW` answers a packet, so the new VM
    /// hears none, and the sweep's petition leg (`0x490b11` → `0x4f3eb0`) frees the signers with
    /// no event; the new VM reads none.
    #[test]
    fn a_reload_replays_no_petition_show_and_frees_the_signers() {
        use crate::ui_script::sweep_test::{reload_in_place, seen, seen_in, spied_vm};
        use crate::ui_script::SweepApp;
        const EVENTS: [&str; 2] = ["PETITION_SHOW", "PETITION_CLOSED"];
        let (commands, _rx) = commands();
        let mut names = NameCache::default();
        names.insert_player(0xbb, "Signer".to_string(), None);
        names.insert_player(0xaa, "Owner".to_string(), None);
        let mut petition = PetitionState::default();
        petition.show(signatures(0x99, 0xaa, 7, &[0xbb]), &commands);
        petition.apply_record(PetitionQueryResponse {
            petition_id: 7,
            name: "Legacy".into(),
            min_signatures: 9,
            ..Default::default()
        });
        let mut app = App::new();
        app.insert_resource(petition)
            .insert_resource(names)
            .insert_resource(commands)
            .init_resource::<GuildRegistrarState>()
            .init_resource::<crate::net::SelfGuid>()
            .init_resource::<crate::ui_chat::ChatLog>()
            .init_resource::<crate::sound::MessageSounds>()
            .add_sweep_leg(
                SweepLeg::Petition,
                feed::free_petition_signers_on_leave_world,
            )
            .add_systems(Update, feed::feed_petition);
        app.insert_non_send_resource(spied_vm(&EVENTS));
        app.update();
        assert_eq!(seen_in(&app), ["PETITION_SHOW"]);

        let dying = reload_in_place(&mut app, &EVENTS);
        assert_eq!(seen(&dying), ["PETITION_SHOW"], "the free fires nothing");
        app.update();
        app.update();
        assert!(seen_in(&app).is_empty(), "no replay into the new VM");
        assert_eq!(
            app.world()
                .non_send_resource::<benilla_ui::script::UiScript>()
                .eval::<i64>("return GetNumPetitionNames()")
                .unwrap(),
            0
        );
    }
}
