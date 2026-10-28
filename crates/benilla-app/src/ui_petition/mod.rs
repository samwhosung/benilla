//! The guild-charter session: the registrar window, the petition window, and the verbs that
//! found a guild. The getters read the charter through the host as they run ([`live`]), and the
//! handlers and the record and name callbacks signal on the charter lane ([`lane`]).
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
use crate::query_cache::Ask;
use crate::query_cache::QueryCache;
use crate::ui_script::{SweepApp, SweepLeg};
use crate::ui_session::{LatchApp, LatchedWindow, NpcWindow};

#[cfg(test)]
mod call_tests;
pub(crate) mod lane;
mod lines;
pub(crate) mod live;
mod steps;

pub(crate) use lane::{PetitionEvent, PetitionLane};
pub(crate) use live::PetitionLive;
pub(crate) use steps::PetitionSteps;

/// The registrar's cells, apart from [`PetitionState`] so a close of the registrar leaves the
/// charter: its NPC (`[0xbdceb0]`), which a close zeroes (`0x4f502b`), and the showlist's first
/// entry's cost (`[0xbdce50]`), which only the next list and the world entry write (`0x4f4ffc`,
/// `0x4f4f60`).
#[derive(Resource, Default)]
pub(crate) struct GuildRegistrarState {
    /// What `BuyGuildCharter` addresses: the petition module's own latch (`[0xbdceb0]`).
    npc: Option<u64>,
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
        self.npc = Some(list.npc);
        self.cost = list.entries[0].charter_cost as u32;
        true
    }

    /// An open registrar on `npc`, past the gates [`Self::open`] runs, for a test.
    #[cfg(test)]
    pub(crate) fn open_for_test(&mut self, npc: u64) {
        self.npc = Some(npc);
    }

    /// `GetGuildCharterCost()` (`0x4f5040`): the last list's, closed or not; 0 before any.
    fn cost(&self) -> u32 {
        self.cost
    }

    /// The registrar's NPC, the cell `[0xbdceb0]`; `None` when it is closed.
    pub(crate) fn npc(&self) -> Option<u64> {
        self.npc
    }

    /// The close core's clear of the cell (`0x4f502b`-`0x4f5030`), the cost kept.
    fn clear(&mut self) {
        self.npc = None;
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
}

/// The close core `0x4f5010` as `0x493310` and the leave-world sweep enter it: with a registrar
/// open, the latch release (`0x4f5024`), the cell cleared, then `GUILD_REGISTRAR_CLOSED`
/// (`0x4f503a`), held behind what the lane still holds ([`lane::behind_lane`]).
/// `CloseGuildRegistrar` and `GetTabardInfo` run it inside their calls
/// ([`crate::script_calls::ScriptCallHost`]).
pub(crate) fn close_core(world: &mut World) {
    if world.resource::<GuildRegistrarState>().npc().is_none() {
        return;
    }
    lane::behind_lane(world, |world| {
        let Some(npc) = world.resource::<GuildRegistrarState>().npc() else {
            return;
        };
        crate::ui_session::release(world, npc);
        world.resource_mut::<GuildRegistrarState>().clear();
        crate::ui_session::fire(world, |script| {
            script.fire_event("GUILD_REGISTRAR_CLOSED", vec![]);
        });
    });
}

/// `SMSG_PETITION_SHOWLIST` past its gates ([`GuildRegistrarState::open`]) opens the registrar
/// through `0x4f4fb0`: the latch first, with no test of the cell (`0x4f4fc0`), then the cell and
/// the entry (`0x4f4fdc`, `0x4f4ffc`), then `GUILD_REGISTRAR_SHOW` (`0x4f5003`), on the lane.
pub(crate) fn open_registrar(world: &mut World, list: &PetitionShowList, npc_flags: Option<u32>) {
    if !GuildRegistrarState::passes(list, npc_flags) {
        return;
    }
    crate::ui_session::register(world, list.npc);
    world
        .resource_mut::<GuildRegistrarState>()
        .open(list, npc_flags);
    if let Some(mut lane) = world.get_resource_mut::<PetitionLane>() {
        lane.push(PetitionEvent::RegistrarShow);
    }
}

/// The petition window's module state (`0xbdce10`-`0xbdce28`, the signer array at `0xbdcdcc`),
/// what the handlers, the callbacks and the charter verbs write.
#[derive(Resource, Default)]
pub(crate) struct PetitionState {
    /// The open charter, `[0xbdce10]` and `[0xbdce18]`.
    open: Option<OpenCharter>,
    /// `[0xbdce1c]`: set while our sign is outstanding. A close's decline leg is gated on it
    /// (`0x4f3f89`), so closing while our own signature is in flight sends no decline.
    signing: bool,
    /// The signers in wire order, then each appended one (`[0xbdce20]`, `0x4f41c0`).
    signers: Vec<u64>,
    /// The signer names still being fetched (`[0xbdce24]`), each a callback `0x4f3f50` kept in
    /// [`Self::name_waits`].
    pending: u32,
    /// The signers whose name lookups keep the window's callback `0x4f3f50`, by guid.
    name_waits: Vec<u64>,
    /// The cached record the window holds (`[0xbdce28]`), by its petition id: written by the open's
    /// lookup (`0x4f406c`) and the record callback's (`0x4f439d`), dropped by a close
    /// (`0x4f4078`), so a close's `PETITION_CLOSED` still reads the old record.
    record: Option<u32>,
    /// The petition ids whose record lookups keep the window's callback `0x4f4090`, one per open
    /// that missed (flag 0, `0x4f4050`); a close leaves them (`0x556e20`).
    record_waits: Vec<u32>,
}

/// The charter the petition window is showing.
#[derive(Clone, Copy, Debug)]
struct OpenCharter {
    /// The charter item's guid, the handle every verb in the family takes.
    item: u64,
    petition_id: u32,
    /// Where the replace wrote it: an item stamped before fires where `[0xbdce10]` is still zero
    /// (`0x4f4020`), so a `ClosePetition` its listener calls finds no charter.
    opened: crate::lanes::Stamp,
}

/// One petition's record, from `SMSG_PETITION_QUERY_RESPONSE`.
#[derive(Clone, Debug, Default)]
struct Record {
    /// The record's owner (`+0x8`), not the packet's: `GetPetitionInfo`'s `isOriginator`
    /// compares against it (`0x4f447a`/`0x4f4481`), as do `CanSignPetition`'s owner refusal and
    /// the close's decline test (`0x4f3fa2`).
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

/// The petition-record cache (`0xc0e0fc`), apart from [`PetitionState`] so the item feeds that read
/// a charter's tooltip beside the VM can share the call host.
#[derive(Resource, Default)]
pub(crate) struct PetitionRecords {
    /// The record cache by petition id, asked once through [`QueryCache`].
    records: QueryCache<u32, Record>,
}

impl PetitionRecords {
    /// Look the record up as `ask` says; whether the cache holds it.
    fn request_record(&self, petition_id: u32, item: u64, ask: Ask<'_>) -> bool {
        self.records
            .read(petition_id, ask, |commands| {
                let _ = commands
                    .0
                    .send(ClientCommand::PetitionQuery { petition_id, item });
            })
            .is_some()
    }

    /// The world entry's clear ([`crate::wdb`]): the petition store is built with persistence off
    /// (`0x554e60`), so `0x555740` clears it (`0x55580c`).
    pub(crate) fn clear_world_session(&mut self) {
        self.records.clear();
    }

    /// Files a record as its answer does, for a test elsewhere.
    #[cfg(test)]
    pub(crate) fn answer_for_test(&mut self, response: PetitionQueryResponse) {
        self.apply_record(response);
    }

    /// Whether the store holds a record for `petition_id`, for a test elsewhere.
    #[cfg(test)]
    pub(crate) fn holds_for_test(&self, petition_id: u32) -> bool {
        self.records.get(petition_id).is_some()
    }

    /// `SMSG_PETITION_QUERY_RESPONSE`: fill the cache.
    fn apply_record(&mut self, response: PetitionQueryResponse) {
        self.records.insert(
            response.petition_id,
            Record {
                owner: response.owner,
                name: response.name,
                body_text: response.body_text,
                required: response.min_signatures as i32,
                is_charter: response.flags & 1 != 0,
            },
        );
    }

    /// The charter tooltip's line 3, the guild name and its master, `None` for an unseen charter
    /// until the answer repaints it.
    pub(crate) fn tooltip_view(
        &self,
        petition_id: u32,
        item: u64,
        names: &NameCache,
        ask: Ask<'_>,
    ) -> Option<benilla_ui::script::PetitionSlotView> {
        if petition_id == 0 {
            return None;
        }
        // The build's lookups pass the callback `0x52e530` (`0x52b93b`, the owner `0x52b9d2`).
        self.request_record(petition_id, item, ask);
        let rec = self.records.get(petition_id)?;
        let (is_charter, title, owner_guid) = (rec.is_charter, rec.name.clone(), rec.owner);
        Some(benilla_ui::script::PetitionSlotView {
            is_charter,
            title,
            owner: names.resolve_player(owner_guid, ask).map(str::to_string),
            // The item object's, which the slot resolve fills.
            signatures: 0,
        })
    }
}

/// What the window's handlers read and send through beside its state: the caches it looks up,
/// the commands its lookups and its decline send, our guid, and the lane it signals on.
pub(crate) struct Window<'a> {
    pub(crate) records: &'a PetitionRecords,
    pub(crate) names: &'a NameCache,
    pub(crate) commands: &'a NetCommands,
    pub(crate) me: u64,
    pub(crate) lane: &'a mut PetitionLane,
}

impl PetitionState {
    /// `0x4f3ef0`: with names pending, each signer's callback `0x4f3f50` cancelled (`0x4f3f29`),
    /// then no signer and none pending.
    fn free_signers(&mut self) {
        self.name_waits.clear();
        self.signers.clear();
        self.pending = 0;
    }

    /// The decline `0x4f3f60` sends for the open petition (`0x4f3fb5`): only with no sign of ours
    /// in flight (`0x4f3f89`), a record held (`0x4f3f95`) and its owner (`+0x8`) not us
    /// (`0x4f3fa5`).
    fn decline(&self, records: &PetitionRecords, me: u64, commands: &NetCommands) {
        let Some(open) = self.open else {
            return;
        };
        let owner = self
            .record
            .and_then(|id| records.records.get(id))
            .map(|r| r.owner);
        if self.signing || self.record.is_none() || owner == Some(me) {
            return;
        }
        let _ = commands
            .0
            .send(ClientCommand::PetitionDecline { item: open.item });
    }

    /// `0x4f3f60` up to its signal: with a petition open, the decline, the signers freed and the
    /// item zeroed (`0x4f3fb5`-`0x4f4026`); whether one was open, for its `PETITION_CLOSED`
    /// (`0x4f402c`), which still reads the record.
    fn close_open(&mut self, records: &PetitionRecords, me: u64, commands: &NetCommands) -> bool {
        if self.open_item().is_none() {
            return false;
        }
        self.decline(records, me, commands);
        self.free_signers();
        self.open = None;
        true
    }

    /// `0x4f3f60`'s close with no petition after its signal (`0x4f4078`): no record, no sign in
    /// flight.
    fn drop_record(&mut self) {
        if self.ahead_of_firing() {
            return;
        }
        self.record = None;
        self.signing = false;
    }

    /// `0x4f3f60(item, id)`, the show's replace: an open petition closes ([`Self::close_open`],
    /// then `PETITION_CLOSED` on the lane), then the new one's cells and its record looked up with
    /// the callback `0x4f4090` (`0x4f4061`), which a miss keeps, and the sign latch cleared.
    fn replace(&mut self, item: u64, petition_id: u32, window: &mut Window<'_>) {
        if self.close_open(window.records, window.me, window.commands) {
            window.lane.push(PetitionEvent::Closed);
        }
        self.signing = false;
        if item == 0 {
            self.record = None;
            return;
        }
        self.open = Some(OpenCharter {
            item,
            petition_id,
            opened: crate::lanes::stamp(),
        });
        let held = window
            .records
            .request_record(petition_id, item, Ask::Event(window.commands));
        self.record = held.then_some(petition_id);
        if !held {
            self.record_waits.push(petition_id);
        }
    }

    /// `0x4f40b0`: the signers stored, each looked up with the callback `0x4f3f50` (`0x4f417d`)
    /// and a miss counted pending (`0x4f4186`); then `PETITION_SHOW` with none pending and a record
    /// held (`0x4f419b`-`0x4f41b4`).
    fn store_signers(&mut self, signers: Vec<u64>, window: &mut Window<'_>) {
        self.free_signers();
        for &signer in &signers {
            if window
                .names
                .resolve_player(signer, Ask::Event(window.commands))
                .is_none()
            {
                self.pending += 1;
                self.name_waits.push(signer);
            }
        }
        self.signers = signers;
        if self.pending == 0 && self.record.is_some() {
            window.lane.push(PetitionEvent::Show);
        }
    }

    /// `0x4f41c0`, another's signature on the open charter: a signer already listed, or another
    /// charter, does nothing; else the signer appended and looked up with the callback, its line
    /// `ERR_PETITION_SIGNED_S` when cached (`0x4f42f6`), else one more pending (`0x4f42e8`); then
    /// `PETITION_SHOW` with none pending, a record or none (`0x4f42fe`-`0x4f430c`).
    fn append_signer(&mut self, item: u64, signer: u64, window: &mut Window<'_>) {
        if self.open.is_none_or(|o| o.item != item) || self.signers.contains(&signer) {
            return;
        }
        self.signers.push(signer);
        match window
            .names
            .resolve_player(signer, Ask::Event(window.commands))
        {
            Some(name) => window
                .lane
                .push(PetitionEvent::Line(lines::signed_by_other(name))),
            None => {
                self.pending += 1;
                self.name_waits.push(signer);
            }
        }
        if self.pending == 0 {
            window.lane.push(PetitionEvent::Show);
        }
    }

    /// The name callback `0x4f3f50` → `0x4f4320` of each signer `guid`'s lookup, whatever the
    /// answer's status: one fewer pending, and `PETITION_SHOW` on the step to none with a record
    /// held (`0x4f432f`-`0x4f433f`).
    fn name_answered(&mut self, guid: u64, lane: &mut PetitionLane) {
        let before = self.name_waits.len();
        self.name_waits.retain(|&g| g != guid);
        for _ in self.name_waits.len()..before {
            if self.pending == 0 {
                continue;
            }
            self.pending -= 1;
            if self.pending == 0 && self.record.is_some() {
                lane.push(PetitionEvent::Show);
            }
        }
    }

    /// The record callback `0x4f4090` of each lookup of `petition_id` that missed, on a found
    /// record: `0x4f4350` takes the record when it is the open petition's (`0x4f436e`,
    /// `0x4f439d`), then `PETITION_SHOW` with none pending and a record held (`0x4f43ad`).
    fn record_answered(&mut self, petition_id: u32, lane: &mut PetitionLane) {
        let before = self.record_waits.len();
        self.record_waits.retain(|&id| id != petition_id);
        for _ in self.record_waits.len()..before {
            if self.open.is_none_or(|o| o.petition_id != petition_id) {
                continue;
            }
            self.record = Some(petition_id);
            if self.pending == 0 {
                lane.push(PetitionEvent::Show);
            }
        }
    }

    /// The session end, with no decline. The record cache stays: it is keyed by the server's
    /// petition id, and the world-enter release re-arms its asks.
    fn clear_session(&mut self) {
        *self = Self::default();
    }

    /// The sweep's free of the signer list (`0x4f3eb0`), which fires nothing.
    fn free_signer_list(&mut self) {
        self.signers.clear();
        self.name_waits.clear();
        self.pending = 0;
    }

    /// What `SignPetition`, `OfferPetition` and `RenamePetition` address: none at the moment of
    /// an item stamped before the replace that opened it.
    fn open_item(&self) -> Option<u64> {
        self.open
            .filter(|_| !self.ahead_of_firing())
            .map(|o| o.item)
    }

    /// Whether the item firing now was stamped before the open charter's replace wrote it.
    fn ahead_of_firing(&self) -> bool {
        self.open
            .is_some_and(|o| crate::lanes::firing_now().is_some_and(|at| at < o.opened))
    }
}

/// The leave-world sweep's petition leg (`0x490b11` → `0x4f3eb0`): the signer list is freed, with
/// no event; the charter window stays.
fn free_petition_signers_on_leave_world(mut petition: ResMut<PetitionState>) {
    petition.free_signer_list();
}

/// The two resources, the registrar on the latch, and the lane.
pub(crate) struct UiPetitionPlugin;

impl Plugin for UiPetitionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        lane::add_petition_lane(app);
        app.init_resource::<GuildRegistrarState>()
            .init_resource::<PetitionState>()
            .init_resource::<PetitionRecords>()
            .add_latched_window::<GuildRegistrarState>()
            // `0x490b07` → `0x4f4fa0` → `0x4f5010`, the core.
            .add_sweep_leg(SweepLeg::Registrar, close_core)
            .add_sweep_leg(SweepLeg::Petition, free_petition_signers_on_leave_world);
    }
}

/// The inbound arms, one per packet, each signalling on the charter lane inside itself.
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    use crate::net::{GuidIndex, ObjectStore, Objects, SelfGuid};
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
            // The signers' name callbacks, as a second handler on the name answer.
            .net_handler(K::PlayerName, on_player_name)
            .net_handler(K::Connected, on_world_enter)
            .net_handler(K::Worldport, on_world_enter)
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

    /// The world-enter step's zero of the showlist's entry (`0x4f4f60`), its cost among it.
    fn on_world_enter(In(ev): In<SessionEvent>, mut registrar: ResMut<GuildRegistrarState>) {
        if matches!(
            ev,
            SessionEvent::Connected { .. } | SessionEvent::Worldport { .. }
        ) {
            registrar.cost = 0;
        }
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

    /// What the window's handlers hold beside its state.
    #[derive(bevy::ecs::system::SystemParam)]
    pub(crate) struct WindowParam<'w> {
        petition: ResMut<'w, PetitionState>,
        records: Res<'w, PetitionRecords>,
        names: Res<'w, NameCache>,
        commands: Res<'w, NetCommands>,
        me: Res<'w, SelfGuid>,
        lane: ResMut<'w, PetitionLane>,
    }

    impl WindowParam<'_> {
        fn run(&mut self, f: impl FnOnce(&mut PetitionState, &mut Window<'_>)) {
            let mut window = Window {
                records: &self.records,
                names: &self.names,
                commands: &self.commands,
                me: self.me.0.unwrap_or(0),
                lane: &mut self.lane,
            };
            f(&mut self.petition, &mut window);
        }
    }

    /// The ignore list is consulted before anything else (`0x5eeefe`).
    fn on_show_signatures(
        In(ev): In<SessionEvent>,
        mut window: WindowParam,
        social: Res<SocialState>,
    ) {
        if let SessionEvent::PetitionShowSignatures(sigs) = ev {
            let ignored = social.is_ignored(sigs.owner);
            window.run(|petition, window| show_signatures(petition, sigs, ignored, window));
        }
    }

    fn on_query_response(
        In(ev): In<SessionEvent>,
        mut records: ResMut<PetitionRecords>,
        mut petition: ResMut<PetitionState>,
        mut lane: ResMut<PetitionLane>,
    ) {
        if let SessionEvent::PetitionQueryResponse(response) = ev {
            let id = response.petition_id;
            query_response(&mut records, response);
            // The answer files the record, then runs the callbacks waiting on it (`0x556f91`).
            petition.record_answered(id, &mut lane);
        }
    }

    /// The name answer runs the window's callbacks waiting on the guid, after the cache filed it
    /// (`0x55f426`-`0x55f452`).
    fn on_player_name(
        In(ev): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        mut lane: ResMut<PetitionLane>,
    ) {
        if let SessionEvent::PlayerName { guid, .. } = ev {
            petition.name_answered(guid, &mut lane);
        }
    }

    fn on_sign_results(In(ev): In<SessionEvent>, mut window: WindowParam) {
        if let SessionEvent::PetitionSignResults(results) = ev {
            window.run(|petition, window| sign_results(petition, results, window));
        }
    }

    fn on_turn_in_results(In(ev): In<SessionEvent>, mut lane: ResMut<PetitionLane>) {
        if let SessionEvent::TurnInPetitionResults { result } = ev {
            turn_in_results(result, &mut lane);
        }
    }

    fn on_declined(
        In(ev): In<SessionEvent>,
        names: Res<NameCache>,
        mut lane: ResMut<PetitionLane>,
    ) {
        if let SessionEvent::PetitionDeclined { player } = ev {
            declined(&names, player, &mut lane);
        }
    }

    fn on_renamed(
        In(ev): In<SessionEvent>,
        objects: Objects,
        items: Res<crate::items::Items>,
        mut records: ResMut<PetitionRecords>,
        mut lane: ResMut<PetitionLane>,
    ) {
        if let SessionEvent::PetitionRenamed(rename) = ev {
            renamed(&objects, &items, &mut records, rename, &mut lane);
        }
    }

    /// `SMSG_PETITION_SHOW_SIGNATURES` (`0x5eee60`): our charter or one offered to us. An ignored
    /// owner suppresses the whole update, silently (`0x5eeefe`-`0x5eef0b`); else the replace
    /// (`0x5eef1a` → `0x4f3f60`) and the signers' store (`0x5eef29` → `0x4f40b0`).
    pub(crate) fn show_signatures(
        petition: &mut PetitionState,
        sigs: PetitionShowSignatures,
        owner_ignored: bool,
        window: &mut Window<'_>,
    ) {
        if owner_ignored {
            debug!(
                "ui_petition: charter from ignored owner {:#x} — whole update dropped",
                sigs.owner
            );
            return;
        }
        petition.replace(sigs.item, sigs.petition_id, window);
        let signers = sigs.signatures.into_iter().map(|s| s.signer).collect();
        petition.store_signers(signers, window);
    }

    /// `SMSG_PETITION_QUERY_RESPONSE`: the record cache fill.
    pub(crate) fn query_response(records: &mut PetitionRecords, response: PetitionQueryResponse) {
        records.apply_record(response);
    }

    /// `SMSG_PETITION_SIGN_RESULTS` (`0x5eefc0`): signer and owner get identical copies, told apart
    /// by the player guid (`0x5eeff6`). Another's signature is appended, result unread
    /// (`0x4f41c0`); ours shows its line, and on success `PETITION_CLOSED` after it (`0x5ef025`,
    /// `0x5ef037`), the window and the sign latch left for its `ClosePetition`.
    pub(crate) fn sign_results(
        petition: &mut PetitionState,
        results: PetitionSignResults,
        window: &mut Window<'_>,
    ) {
        if results.player != window.me {
            petition.append_signer(results.item, results.player, window);
            return;
        }
        if let Some(line) = lines::my_sign_line(results.result) {
            window.lane.push(PetitionEvent::Line(line));
        }
        if results.result == petition_result::OK {
            window.lane.push(PetitionEvent::Closed);
        }
    }

    /// `SMSG_TURN_IN_PETITION_RESULTS` (`0x5ef150`): success prints nothing and signals
    /// `GUILD_REGISTRAR_CLOSED` bare (`0x5ef16b`), with the cell and the latch left for the frame's
    /// `OnHide`, whose `CloseGuildRegistrar()` runs the core. Only the two refusals print, both
    /// red; the petition window is not touched.
    pub(crate) fn turn_in_results(result: u32, lane: &mut PetitionLane) {
        if result == petition_result::OK {
            lane.push(PetitionEvent::RegistrarClosed);
            return;
        }
        if let Some(line) = lines::turn_in_line(result) {
            lane.push(PetitionEvent::Line(line));
        }
    }

    /// `MSG_PETITION_DECLINE` inbound: a line only if the decliner's name is already cached, with
    /// no query or retry (`0x5ef12a`/`0x5ef139`), so this arm takes no command channel.
    pub(crate) fn declined(names: &NameCache, player: u64, lane: &mut PetitionLane) {
        if let Some(name) = names.resolve_unit(player, None, Ask::Read) {
            lane.push(PetitionEvent::Line(lines::declined_line(name)));
        }
    }

    /// `MSG_PETITION_RENAME` inbound (`0x5ef1d0`): an empty name does nothing (`0x5ef20a`); else
    /// the item the guid names, a charter by its template's flag (`0x5ef240`), its petition id off
    /// enchantment slot 0 (`0x5ef248`), and that petition's cached record, looked up with no
    /// callback (`0x5ef27c`): the name overwrites its title in place (`0x5ef292`) and
    /// `PETITION_SHOW` follows (`0x5ef29c`), whatever the window holds.
    pub(crate) fn renamed(
        objects: &Objects,
        items: &crate::items::Items,
        records: &mut PetitionRecords,
        rename: PetitionRename,
        lane: &mut PetitionLane,
    ) {
        if rename.name.is_empty() {
            return;
        }
        let Some(fields) = objects.object(rename.item) else {
            return;
        };
        let charter = fields
            .object_entry()
            .and_then(|entry| items.template(entry, rename.item, Ask::Read))
            .is_some_and(|t| t.flags & benilla_protocol::messages::ITEM_FLAG_CHARTER != 0);
        let id = fields.item_enchant(0).unwrap_or(0) as u32;
        if !charter || id == 0 {
            return;
        }
        let Some(record) = records.records.get_mut(id) else {
            return;
        };
        record.name = rename.name;
        lane.push(PetitionEvent::Show);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::{PetitionShowListEntry, PetitionSignature};
    use crossbeam_channel::unbounded;

    const ME: u64 = 0x10;

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

    /// `sigs` through the show's arm, as us; what it put on the lane.
    fn show(
        petition: &mut PetitionState,
        records: &PetitionRecords,
        names: &NameCache,
        commands: &NetCommands,
        sigs: PetitionShowSignatures,
    ) -> Vec<PetitionEvent> {
        let mut lane = PetitionLane::default();
        let mut window = Window {
            records,
            names,
            commands,
            me: ME,
            lane: &mut lane,
        };
        net::show_signatures(petition, sigs, false, &mut window);
        lane.stamped().into_iter().map(|(_, e)| e).collect()
    }

    /// The open charter's cached title, `GetPetitionInfo`'s second return.
    fn open_title<'a>(p: &PetitionState, r: &'a PetitionRecords) -> Option<&'a str> {
        let id = p.record?;
        r.records.get(id).map(|r| r.name.as_str())
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
        r.clear();
        assert_eq!(r.cost(), 1000, "a close keeps the entry (`0x4f502b`)");

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
            .init_resource::<PetitionLane>()
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
        let mut petition = PetitionState::default();
        show(
            &mut petition,
            &PetitionRecords::default(),
            &NameCache::default(),
            &commands,
            signatures(0x99, 0xaa, 7, &[0xbb]),
        );
        world.insert_resource(petition);

        crate::ui_session::dispatch(world);
        assert_eq!(world.resource::<GuildRegistrarState>().npc(), None);
        assert_eq!(
            world.resource::<PetitionState>().open_item(),
            Some(0x99),
            "the charter window is item-bound and survives the walk-away"
        );
    }

    /// The open's record lookup sends the query once while it is in flight; the answer runs the
    /// callback each open that missed kept, which takes the record of the petition still open
    /// (`0x4f4350`). Each open's replace closes the one before (`0x4f402c`).
    #[test]
    fn the_record_query_is_asked_once_and_its_answer_shows_the_open_petition() {
        let (commands, rx) = commands();
        let names = NameCache::default();
        let mut petition = PetitionState::default();
        let mut records = PetitionRecords::default();
        assert!(show(
            &mut petition,
            &records,
            &names,
            &commands,
            signatures(0x99, 0xaa, 7, &[])
        )
        .is_empty());
        assert_eq!(
            show(
                &mut petition,
                &records,
                &names,
                &commands,
                signatures(0x99, 0xaa, 7, &[])
            ),
            [PetitionEvent::Closed]
        );
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

        // The answer files the record, then runs both callbacks.
        net::query_response(
            &mut records,
            PetitionQueryResponse {
                petition_id: 7,
                name: "Legacy".into(),
                min_signatures: 9,
                ..Default::default()
            },
        );
        let mut lane = PetitionLane::default();
        petition.record_answered(7, &mut lane);
        assert_eq!(
            lane.stamped()
                .into_iter()
                .map(|(_, e)| e)
                .collect::<Vec<_>>(),
            [PetitionEvent::Show, PetitionEvent::Show]
        );
        assert_eq!(open_title(&petition, &records), Some("Legacy"));
        // A held record shows at the open itself (`0x4f41b4`), with no query.
        assert_eq!(
            show(
                &mut petition,
                &records,
                &names,
                &commands,
                signatures(0x99, 0xaa, 7, &[])
            ),
            [PetitionEvent::Closed, PetitionEvent::Show]
        );
        assert!(
            !rx.try_iter()
                .any(|c| matches!(c, ClientCommand::PetitionQuery { .. })),
            "still no second query"
        );
    }

    /// `PETITION_SHOW` waits for every signer name (`0x4f419b`): each missed name keeps the
    /// callback `0x4f3f50`, and the last one to answer, a refusal too, ends the wait
    /// (`0x4f4320`), once.
    #[test]
    fn the_show_waits_for_the_last_signer_name() {
        let (commands, _rx) = commands();
        let mut names = NameCache::default();
        let mut records = PetitionRecords::default();
        net::query_response(
            &mut records,
            PetitionQueryResponse {
                petition_id: 7,
                name: "Legacy".into(),
                ..Default::default()
            },
        );
        let mut petition = PetitionState::default();
        assert!(show(
            &mut petition,
            &records,
            &names,
            &commands,
            signatures(0x99, 0xaa, 7, &[0xbb, 0xcc])
        )
        .is_empty());
        assert_eq!(petition.pending, 2);
        let mut lane = PetitionLane::default();
        names.insert_player(0xbb, "Bee".into(), None);
        petition.name_answered(0xbb, &mut lane);
        assert!(lane.stamped().is_empty());
        names.insert_player(0xcc, String::new(), None);
        petition.name_answered(0xcc, &mut lane);
        petition.name_answered(0xcc, &mut lane);
        assert_eq!(
            lane.stamped()
                .into_iter()
                .map(|(_, e)| e)
                .collect::<Vec<_>>(),
            [PetitionEvent::Show]
        );
    }

    /// `MSG_PETITION_RENAME` (`0x5ef1d0`) patches the cached record of the petition the charter
    /// item's enchantment slot 0 names, then signals `PETITION_SHOW` (`0x5ef29c`); an item that is
    /// no charter, or a record not cached, takes nothing.
    #[test]
    fn a_rename_echo_patches_the_charter_items_cached_record() {
        use benilla_protocol::ObjectFields;
        const CHARTER: u64 = 0xC1;
        let mut objs = crate::ui_items::TestObjects::new();
        // OBJECT_FIELD_ENTRY 3; the petition id at 22, enchantment slot 0.
        objs.spawn(CHARTER, ObjectFields::from_pairs(&[(3, 5863), (22, 7)]));
        objs.spawn(0xC2, ObjectFields::from_pairs(&[(3, 25), (22, 7)]));
        let mut items = crate::items::Items::default();
        let mut charter = crate::items::test_template("Guild Charter");
        charter.flags = benilla_protocol::messages::ITEM_FLAG_CHARTER;
        items.insert_template(5863, Some(charter));
        items.insert_template(25, Some(crate::items::test_template("Worn Shortsword")));
        let mut records = PetitionRecords::default();
        let mut lane = PetitionLane::default();
        let rename = |item: u64, name: &str| PetitionRename {
            item,
            name: name.into(),
        };
        net::renamed(
            &objs.get(),
            &items,
            &mut records,
            rename(CHARTER, "Early"),
            &mut lane,
        );
        assert!(lane.stamped().is_empty(), "no record cached");
        net::query_response(
            &mut records,
            PetitionQueryResponse {
                petition_id: 7,
                name: "First".into(),
                ..Default::default()
            },
        );
        net::renamed(
            &objs.get(),
            &items,
            &mut records,
            rename(0xC2, "Wrong"),
            &mut lane,
        );
        net::renamed(
            &objs.get(),
            &items,
            &mut records,
            rename(CHARTER, ""),
            &mut lane,
        );
        assert!(lane.stamped().is_empty(), "no charter, or no name");
        net::renamed(
            &objs.get(),
            &items,
            &mut records,
            rename(CHARTER, "Second"),
            &mut lane,
        );
        assert_eq!(
            records.records.get(7).map(|r| r.name.as_str()),
            Some("Second")
        );
        assert_eq!(
            lane.stamped()
                .into_iter()
                .map(|(_, e)| e)
                .collect::<Vec<_>>(),
            [PetitionEvent::Show]
        );
    }

    /// Through the real registration: a new login's VM never runs the old one's `OnHide`.
    #[test]
    fn the_session_end_closes_the_charter_and_forgets_the_sign() {
        let (commands, _rx) = commands();
        let mut app = App::new();
        app.add_plugins(UiPetitionPlugin);
        let mut petition = PetitionState::default();
        show(
            &mut petition,
            &PetitionRecords::default(),
            &NameCache::default(),
            &commands,
            signatures(0x99, 0xaa, 7, &[0xbb]),
        );
        petition.signing = true;
        app.insert_resource(petition);
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

    /// The sweep's petition leg (`0x490b11` → `0x4f3eb0`) frees the signer list and signals
    /// nothing; the charter stays open.
    #[test]
    fn the_sweep_frees_the_signers_and_keeps_the_charter() {
        let (commands, _rx) = commands();
        let mut petition = PetitionState::default();
        show(
            &mut petition,
            &PetitionRecords::default(),
            &NameCache::default(),
            &commands,
            signatures(0x99, 0xaa, 7, &[0xbb]),
        );
        let mut app = App::new();
        app.insert_resource(petition)
            .init_resource::<PetitionLane>()
            .add_systems(Update, free_petition_signers_on_leave_world);
        app.update();
        let petition = app.world().resource::<PetitionState>();
        assert!(petition.signers.is_empty() && petition.pending == 0);
        assert_eq!(petition.open_item(), Some(0x99));
        assert!(app.world().resource::<PetitionLane>().stamped().is_empty());
    }
}
