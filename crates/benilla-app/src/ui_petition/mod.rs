//! The guild-charter session — the registrar window, the petition window, and the verbs that found
//! a guild.
//!
//! [`crate::ui_guild`] is *being* in a guild; this is *making* one, the slice 1257 §2 deliberately
//! left out and named as the next. It mirrors the wire the way that module does: the seven server
//! packets replace or patch [`PetitionState`] / [`GuildRegistrarState`], and [`feed`] turns them
//! into the display-ready snapshot `benilla_ui::script::petition` reads.
//!
//! **Two windows, two sessions, and only one of them is an NPC session.**
//!
//! - [`GuildRegistrarState`] is the NPC half: it opens on `SMSG_PETITION_SHOWLIST` (which the
//!   server pushes when the "How do I form a guild?" gossip row is selected — vmangos
//!   `Player.cpp:12428-12431` closes the gossip menu and calls `SendPetitionShowList`), it knows a
//!   price and an NPC guid, and it closes when the player walks away like every other NPC window
//!   ([`crate::ui_session`]). The reference corroborates the shape rather than merely permitting
//!   it: the function that fires `GUILD_REGISTRAR_SHOW` (`0x4f4fb0`, firing at `0x4f4fff`) is one
//!   of the fourteen callers of `CGGameUI::SetInteractNPC 0x4930d0`, which is the *same* latch
//!   every other NPC window arms.
//! - [`PetitionState`] is the item half: it opens on `SMSG_PETITION_SHOW_SIGNATURES`, is bound to a
//!   charter **item guid**, and has no NPC at all. Walking away from the registrar must not close
//!   it, which is why the two are separate resources rather than one with a half-meaning `close()`.
//!
//! They meet at exactly one place: `TurnInGuildCharter()` is a *registrar* button that acts on a
//! charter in the *bags*, found by [`crate::ui_items::find_item`].
//!
//! **The gossip handoff needs no special case, and that is worth saying because the banker's did**.
//! vmangos's `GOSSIP_OPTION_PETITIONER` arm calls `PlayerTalkClass->CloseGossip()`
//! *before* `SendPetitionShowList` (`Player.cpp:12428-12431`), and `CloseGossip` really does send
//! `SMSG_GOSSIP_COMPLETE` (`GossipDef.cpp:231-236`) — so the menu closes itself, and the registrar
//! takes the left panel slot the gossip window has already released. The bank needed
//! `npc::show_bank` to force that close precisely because its option sends no such packet; this one
//! must NOT, or the close would be done twice.
//!
//! **The laws that are not what the obvious design would do:**
//!
//! - **The window opens before it knows what it is showing.**
//!   `SMSG_PETITION_SHOW_SIGNATURES` carries an item guid, an owner guid, a petition id and a list
//!   of signer guids — and no text whatsoever. The proposed guild's name and the signature
//!   requirement come only from `SMSG_PETITION_QUERY_RESPONSE`, keyed by petition id, and the
//!   names come from [`crate::names::NameCache`]. So the petition window's first paint is
//!   deliberately partial and repaints as each lookup lands. This is the identical two-caches shape
//!   `SMSG_GUILD_ROSTER` has with `SMSG_GUILD_QUERY_RESPONSE` ([`crate::ui_guild`]'s own module
//!   doc), and the cache is lazy for the same reason: nothing queries a petition we are not looking
//!   at.
//! - **One inbound packet, two meanings.** `SMSG_PETITION_SHOW_SIGNATURES` answers our own
//!   `CMSG_PETITION_SHOW_SIGNATURES` *and* is what arrives when somebody offers us their charter
//!   (`Handlers/PetitionsHandler.cpp:390-397`). Nothing distinguishes them but whether `owner` is
//!   us — so there is no "an offer arrived" path separate from "a charter opened", and building one
//!   would be inventing a distinction the wire does not make.
//! - **A signature does not push a new list.** vmangos answers a successful sign with
//!   `SMSG_PETITION_SIGN_RESULTS` to both parties and **no fresh signatures packet**
//!   (`:299`, `:312-316`), while the reference's `PetitionFrame` registers only `PETITION_SHOW` and
//!   `PETITION_CLOSED`. Nothing would ever repaint the name rows. So an `OK` result naming the open
//!   charter re-sends `CMSG_PETITION_SHOW_SIGNATURES` ([`net::sign_results`]) — INFERRED, and
//!   flagged as such below.
//! - **Two success paths are silent on the wire and speak locally.** Offering a charter answers the
//!   *target*, not us, and finding no charter to turn in is refused before any packet is built —
//!   so `ERR_PETITION_OFFERED_S` and `ERR_NO_GUILD_CHARTER` are composed here. Both strings exist
//!   in `GlobalStrings.lua` and nothing on the wire can produce either, which is the same test
//!   [`crate::ui_guild::lines`] uses to identify an engine-composed line.
//!
//! ## What is INFERRED, and what would settle it
//!
//! Every claim below is read off the *demand* side (the shipped `PetitionFrame.lua` /
//! `GuildRegistrarFrame.lua`) plus vmangos, and is named here so none of it is mistaken for byte
//! law:
//!
//! 1. **`SMSG_PETITION_SHOWLIST` → `GUILD_REGISTRAR_SHOW`** (no args). The firer `0x4f4fb0` sits in
//!    the petition band with `0x5eeb80` as its sole caller; the byte read of that handler settles
//!    it.
//! 2. **`SMSG_PETITION_SHOW_SIGNATURES` → `PETITION_SHOW`** (no args), everything painted from
//!    getters. The reference's own OnEvent uses no `arg1`, which makes this weak-but-consistent.
//! 3. **`ClosePetition()` / `CloseGuildRegistrar()` send nothing.** Chosen because inventing
//!    traffic 1.12 never sends is the worse failure, and because the neighbouring
//!    `CloseGuildRoster 0x4d2350` sends nothing either (`xor eax,eax; ret`).
//! 4. **Re-requesting the signature list on an `OK` sign result** (above).
//! 5. **`ERR_GUILD_FOUNDER_S` on a successful turn-in.** vmangos declares `GUILD_FOUNDER_S = 0x0E`
//!    and **never sends it** — `Guild::Create` broadcasts no event at all on the founding path
//!    (`Guild/Guild.cpp:104-119`), so without a local line, founding a guild is completely silent.
//!    The string is routed through [`crate::ui_guild::lines`]'s existing constant so there is one
//!    copy; a server that *does* send the command result would double the line, which is visible
//!    and cheap to undo.
//! 6. **Using a charter item opens it** — see [`crate::ui_items::ItemUseRoute::ShowPetition`],
//!    where the evidence and its limits are written down.

use benilla_protocol::messages::{
    petition_result, PetitionQueryResponse, PetitionRename, PetitionShowList,
    PetitionShowSignatures, PetitionSignResults,
};
use bevy::prelude::*;

use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::QueryCache;
use crate::ui_script::{UiFeed, UiInput};
use crate::ui_session::NpcSession;

mod feed;
mod lines;

/// The open guild registrar — an NPC window, and nothing more than a price.
///
/// Deliberately separate from [`PetitionState`]: only this half is bound to an NPC, so only this
/// half may be closed by the walk-away guard. A single resource implementing [`NpcSession`] would
/// have a `close()` that closed half of itself, which is the kind of thing that reads as correct
/// for a year.
#[derive(Resource, Default)]
pub(crate) struct GuildRegistrarState {
    open: Option<Registrar>,
}

/// What one open registrar knows.
#[derive(Clone, Debug)]
struct Registrar {
    /// The NPC — what `BuyGuildCharter` addresses (the petition module's own latched guid
    /// `[0xbdceb0]`, *not* `CGGameUI`'s interaction pair), and what the range guard watches.
    npc: u64,
    /// The charter's price in copper. **Unsigned**, because the binding pushes it through the
    /// zero-extending `fild QWORD` idiom.
    cost: u32,
}

/// The two `UNIT_NPC_FLAGS` bits the client requires on a showlist's NPC before it will open the
/// registrar — bit 9 **and** bit 10, `PETITIONER | TABARDDESIGNER`
/// (`0x5eec33`/`0x5eec3b`, two separate `test`/`je` pairs).
///
/// vmangos gates its *send* on `PETITIONER` alone, so the two disagree — but not observably on this
/// world's data: all six petitioner templates carry both bits, and the eight tabard-only ones never
/// send a showlist at all. Implemented anyway, because it is three lines and a silent-drop gate is
/// exactly the kind of thing that matters the day the data changes.
const REGISTRAR_NPC_FLAGS: u32 = crate::target::cursor_mode::npc_flags::PETITIONER
    | crate::target::cursor_mode::npc_flags::TABARDDESIGNER;

impl GuildRegistrarState {
    /// `SMSG_PETITION_SHOWLIST` — open (or re-open) the registrar, **if it passes all three gates**.
    ///
    /// `npc_flags` is the NPC's live `UNIT_NPC_FLAGS`, or `None` when the guid does not resolve to a
    /// streamed unit — which fails the gate, as the client's own `0x5eec1a` resolve does.
    ///
    /// The three gates, in the showlist handler `0x5eeb80`:
    /// 1. the NPC resolves as a unit and carries [`REGISTRAR_NPC_FLAGS`];
    /// 2. **entry\[0\]**'s `entryFlags & 1` is set (`0x5eec42`, and it is entry\[0\]'s fifth dword
    ///    specifically — no other entry's flags are tested anywhere on this path);
    /// 3. there is an entry\[0\] at all.
    ///
    /// **Entry\[0\] and no other.** The handler parses every entry the count declares — a
    /// re-implementation must too, to stay in sync with the stream — but a whole-image census of the
    /// ten-entry table found exactly four references, all inside the handler, and only entry\[0\] is
    /// ever consumed. This file first took "the first row with the visible flag", which reads as the
    /// same thing on a one-row packet and is a different rule.
    ///
    /// A packet that fails any gate is **parsed and silently dropped** — no window, no error line.
    fn open(&mut self, list: &PetitionShowList, npc_flags: Option<u32>) -> bool {
        if npc_flags.is_none_or(|f| f & REGISTRAR_NPC_FLAGS != REGISTRAR_NPC_FLAGS) {
            debug!(
                "ui_petition: showlist from {:#x} without both registrar npc flags — dropped",
                list.npc
            );
            return false;
        }
        let Some(first) = list.entries.first().filter(|e| e.entry_flags & 1 != 0) else {
            debug!("ui_petition: showlist entry[0] not flagged visible — dropped");
            return false;
        };
        self.open = Some(Registrar {
            npc: list.npc,
            cost: first.charter_cost as u32,
        });
        true
    }

    /// The charter's price in copper, `0` when no registrar is open — `GetGuildCharterCost()`.
    fn cost(&self) -> u32 {
        self.open.as_ref().map_or(0, |r| r.cost)
    }
}

impl NpcSession for GuildRegistrarState {
    fn npc(&self) -> Option<u64> {
        self.open.as_ref().map(|r| r.npc)
    }

    fn close(&mut self) {
        self.open = None;
    }
}

/// The open charter and the petition-record cache.
#[derive(Resource, Default)]
pub(crate) struct PetitionState {
    open: Option<OpenCharter>,
    /// The lazy record cache, keyed by petition id — the guild-identity cache's twin, ask-once
    /// through [`QueryCache`]. Session state: a petition id means nothing across
    /// a reconnect.
    records: QueryCache<u32, Record>,
    /// `[0xbdce1c]` — set while a sign we sent is outstanding, cleared when its result lands.
    ///
    /// It exists for one reader, and that reader is easy to miss: the decline leg of a close is
    /// gated on it (`0x4f3f89`), so closing a window while your own signature is in flight must
    /// **not** put `MSG_PETITION_DECLINE` on the wire.
    signing: bool,
    /// One step per landed or patched petition record — the **gate input** the charter tooltip's
    /// line 3 needs.
    ///
    /// The record cache is lazy: the hover is what issues the query, so the answer arriving sets
    /// nothing any consumer already watches. `ui_items`' bag feed gates its whole snapshot rebuild
    /// on a list of epochs, and without this counter in that list a charter's guild
    /// lines are resolved once — to `None`, before the record exists — and never again. The live
    /// probe is what caught it; every unit test passed, because they push the record and the view
    /// in the same breath.
    records_epoch: u64,
    /// Lines composed at apply time, waiting for the feed — which is where the `script` handle
    /// lives, and so the only place a red `UI_ERROR_MESSAGE` can be fired ([`crate::ui_bank`]'s
    /// `BankErrors` shape).
    lines: Vec<lines::Line>,
}

/// The charter the petition window is showing.
#[derive(Clone, Debug)]
struct OpenCharter {
    /// The charter **item's** guid — the handle every verb in the family takes.
    item: u64,
    /// Its owner. Whether this is us decides which half of the window shows.
    owner: u64,
    /// The petition id — the key into [`PetitionState::records`].
    petition_id: u32,
    /// The signer guids, in wire order. Names are resolved at feed time and are deliberately not
    /// stored: a name that lands later must change the display without any packet re-arriving.
    signers: Vec<u64>,
}

/// One petition's record, from `SMSG_PETITION_QUERY_RESPONSE`.
#[derive(Clone, Debug, Default)]
struct Record {
    /// The charter owner's guid — the record's `+0x8`/`+0xc`.
    ///
    /// **The record's owner, not the packet's**, and the distinction is the client's own:
    /// `GetPetitionInfo`'s `isOriginator` compares the active player against `[edi+0x8]`
    /// (`0x4f447a`/`0x4f4481`), and `CanSignPetition`'s owner refusal reads `[esi+0x8]`. They hold
    /// the same value on any sane server — `SMSG_PETITION_SHOW_SIGNATURES` and
    /// `SMSG_PETITION_QUERY_RESPONSE` both name the owner — but only one of them is the source.
    owner: u64,
    /// The proposed guild's name.
    name: String,
    /// Free text — always empty on a 1.12 server.
    body_text: String,
    /// How many signatures it **requires** — the record's `+0x1118`, and a **signed** i32 because
    /// the binding pushes it with `fild DWORD`. What the Request-Signature button disables against,
    /// and what `CanSignPetition` compares the live count to (`0x4f4634`); distinct from the nine
    /// name *rows*.
    required: i32,
    /// The record's `+0x1110` bit 0 — set for a guild charter, clear for a plain petition. It picks
    /// `GetPetitionInfo`'s first return, and it **gates two of `CanSignPetition`'s refusals**, so a
    /// non-charter petition is never refused for guild membership or for a full list.
    is_charter: bool,
}

impl crate::query_cache::AskOnce for PetitionState {
    fn clear_pending(&mut self) {
        self.records.clear_pending();
    }
}

impl PetitionState {
    /// `SMSG_PETITION_SHOW_SIGNATURES` — open the window on this charter, and fire the record
    /// query if we have never seen this petition.
    ///
    /// Takes the command channel rather than holding one: the same seam
    /// [`crate::ui_guild::GuildState::request_identity`] has, so the state stays plain data.
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

    /// The lazy record fill: ask once per petition id, and answer nothing for the call that missed.
    /// The arrival simply lands in the cache; the feed's next rebuild picks it up (see
    /// [`feed`]'s note on why there is no flag).
    fn request_record(&self, petition_id: u32, item: u64, commands: &NetCommands) {
        self.records.get_or_ask(petition_id, || {
            let _ = commands
                .0
                .send(ClientCommand::PetitionQuery { petition_id, item });
        });
    }

    /// One step per landed or patched record — see [`Self::records_epoch`]'s own field doc.
    pub(crate) fn records_epoch(&self) -> u64 {
        self.records_epoch
    }

    /// `SMSG_PETITION_QUERY_RESPONSE` — fill the cache.
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

    /// The charter tooltip's line-3 view for `petition_id` — the guild name and its master.
    ///
    /// **A lazy cache fill, and the hover is what issues the query.** A charter you have never
    /// opened has no record, so the first hover returns `None` (the plate shows the item's name and
    /// the green `<Right Click for Details>` line and nothing between) and the answer arriving
    /// repaints it. That is [`crate::names::NameCache::resolve`]'s rule, the guild-identity cache's
    /// rule, and the same shape the tooltip's own creator line already has one field up.
    ///
    /// `item` rides along because `CMSG_PETITION_QUERY` carries it; vmangos ignores the field, but
    /// it is sent at its true value like every other field we can fill honestly.
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
            owner: names.resolve(owner_guid, commands).map(str::to_string),
        })
    }

    /// Append one signer to the open charter — the owner's half of a successful sign
    /// (`0x4f41c0`), and it is an **append, not a re-request**.
    ///
    /// The server sends no fresh signature list when somebody signs, and the reference's
    /// `PetitionFrame` registers only `PETITION_SHOW`/`PETITION_CLOSED`. This file first re-sent
    /// `CMSG_PETITION_SHOW_SIGNATURES` to make the rows repaint, which is a round trip the real
    /// client does not make: it grows its own array and fires the event itself.
    ///
    /// Returns the signer's name when the cache already had it — the condition on emitting
    /// `ERR_PETITION_SIGNED_S`. On a miss the name is left unresolved in the list and the feed's own
    /// deferral holds the repaint until it lands, which is the same effect as the client's
    /// pending-name counter.
    ///
    /// A result for a charter we do not have open is not ours to apply.
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
        names.resolve(signer, commands).map(str::to_string)
    }

    /// The decline `ClosePetition()` can put on the wire — `0x4f3f60`'s leg at `0x4f3fb5`.
    ///
    /// **This is the half of `ClosePetition` that is not a local reset**, and it is guarded by four
    /// conditions, every one of which matters:
    ///
    /// 1. a petition is open (`0x4f3f7c`) — otherwise there is nothing to decline;
    /// 2. **no sign of ours is in flight** (`0x4f3f89`) — closing the window while your own
    ///    signature is on the wire must not read as a refusal of the same charter;
    /// 3. a record is cached (`0x4f3f95`) — without one the ownership test below cannot run;
    /// 4. we are **not** the owner (`0x4f3fa5`/`0x4f3fae`) — you do not decline your own charter.
    ///
    /// The same leg runs when a *different* petition replaces the open one, which is why the feed's
    /// switch arm consumes the close intent it causes: without that, being offered a second charter
    /// would silently decline the first.
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

    /// The window's client-side close — no packet (module doc, INFERRED #3).
    fn close(&mut self) {
        self.open = None;
    }

    /// The session end: the open charter, the in-flight sign and the undrained lines are the old
    /// session's and go with it — silently, with **no** decline (the socket is gone, and a
    /// decline is the close verb's, not the teardown's). The record cache stays: it is keyed by
    /// the server's petition id, and the world-enter release already re-arms its asks.
    fn clear_session(&mut self) {
        self.open = None;
        self.signing = false;
        self.lines.clear();
    }

    /// The open charter's item guid — what `SignPetition` / `OfferPetition` / `RenamePetition`
    /// address. `None` = nothing open, and the verb is dropped rather than sent against a guess.
    fn open_item(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.item)
    }
}

/// The plugin: two resources, the walk-away guard on the registrar half, and the feed/drain pair.
pub(crate) struct UiPetitionPlugin;

impl Plugin for UiPetitionPlugin {
    fn build(&self, app: &mut App) {
        net::register(app);
        crate::query_cache::register::<PetitionState>(app);
        app.init_resource::<GuildRegistrarState>()
            .init_resource::<PetitionState>()
            .add_systems(
                Update,
                (
                    // Ahead of the feed, so a walk-away turns into GUILD_REGISTRAR_CLOSED the same
                    // frame — every other NPC window's ordering.
                    crate::ui_session::close_npc_session_out_of_range::<GuildRegistrarState>
                        .before(feed::feed_petition),
                    feed::feed_petition.in_set(UiFeed),
                    feed::drain_petition.after(UiInput),
                ),
            );
    }
}

/// The inbound arms — one per packet, each beside the state it drives.
///
/// **Nothing here fires an event or writes a chat line directly.** Composed lines go onto
/// [`PetitionState::lines`] and the feed drains them, because the red `UI_ERROR_MESSAGE` channel
/// needs the `script` handle and a packet handler has none (`crate::ui_bank`'s `BankErrors`
/// shape). In the net handler table since 2312.
pub(crate) mod net {
    use super::*;
    use benilla_protocol::{SessionEvent, SessionEventKind};

    use crate::net::NetHandlerApp;

    use crate::net::{GuidIndex, ObjectStore, SelfGuid};
    use crate::ui_social::SocialState;

    /// Register the family's handlers — called from [`UiPetitionPlugin`].
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

    /// The charter window and the registrar die with the socket — a listener on the session end
    /// (a second handler on the kind, after the bridge's own teardown). Without it the next
    /// login's fresh VM saw `None → Some` and fired `PETITION_SHOW` for the old session's charter,
    /// and closing that ghost declined it on the wire. The registrar's walk-away guard cannot
    /// stand in: it measures from a self player, and there is none until the next world entry.
    fn on_session_end(
        In(_): In<SessionEvent>,
        mut petition: ResMut<PetitionState>,
        mut registrar: ResMut<GuildRegistrarState>,
    ) {
        petition.clear_session();
        registrar.close();
    }

    /// The registrar's two `UNIT_NPC_FLAGS` gates are on LIVE NPC state rather than on the
    /// packet, so the flags are read here, off the store. An unstreamed guid reads `None` and
    /// fails the gate, as the client's own resolve does.
    fn on_show_list(
        In(ev): In<SessionEvent>,
        mut registrar: ResMut<GuildRegistrarState>,
        index: Res<GuidIndex>,
        stores: Query<&ObjectStore>,
    ) {
        if let SessionEvent::PetitionShowList(list) = ev {
            let flags = index
                .0
                .get(&list.npc)
                .and_then(|e| stores.get(*e).ok())
                .map(|s| s.0.unit_npc_flags());
            show_list(&mut registrar, list, flags);
        }
    }

    /// An ignored owner suppresses the ENTIRE update — no record fetch, no list, no event, no
    /// error line (`0x5eeefe`). Consulted before anything else happens.
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
        mut registrar: ResMut<GuildRegistrarState>,
    ) {
        if let SessionEvent::TurnInPetitionResults { result } = ev {
            turn_in_results(&mut petition, &mut registrar, result);
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

    /// `SMSG_PETITION_SHOWLIST` — the registrar's charter list, subject to its three gates.
    ///
    /// `npc_flags` is the NPC's live `UNIT_NPC_FLAGS`; the caller reads it because only the apply
    /// pass holds the object store.
    pub(crate) fn show_list(
        registrar: &mut GuildRegistrarState,
        list: PetitionShowList,
        npc_flags: Option<u32>,
    ) {
        registrar.open(&list, npc_flags);
    }

    /// `SMSG_PETITION_SHOW_SIGNATURES` — a charter opened, ours or one offered to us.
    ///
    /// **An ignored owner suppresses the whole update** (`0x5eeefe`-`0x5eef0b`): no record fetch, no
    /// signer list, no event, no error line. The player is simply never told. That gate is checked
    /// by the caller, which holds the ignore list.
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

    /// `SMSG_PETITION_QUERY_RESPONSE` — the record cache fill.
    pub(crate) fn query_response(petition: &mut PetitionState, response: PetitionQueryResponse) {
        petition.apply_record(response);
    }

    /// `SMSG_PETITION_SIGN_RESULTS` — and it is **two different code paths in one packet**.
    ///
    /// The server sends a byte-identical copy to the signer and to the charter's owner, and the
    /// client tells them apart by comparing the packet's player guid against its own
    /// (`0x5eefee`). What each does could hardly be less alike:
    ///
    /// - **somebody else signed**: the result is *never inspected*. The signer is **appended to the
    ///   local list** (`0x4f41c0`) — no re-request, which is what this file first did — the count
    ///   rises, and a cached name emits `ERR_PETITION_SIGNED_S` while an uncached one just raises
    ///   the pending counter and stays quiet until it lands.
    /// - **we signed**: the result is switched on for a line, the in-flight latch clears, and a
    ///   success additionally **closes the window** (`0x5ef037` fires `PETITION_CLOSED`).
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

    /// `SMSG_TURN_IN_PETITION_RESULTS` — the verdict on a turn-in.
    ///
    /// **Success says nothing and closes the REGISTRAR** (`0x5ef166` fires
    /// `GUILD_REGISTRAR_CLOSED`), not the charter window — which the destroyed item closes on its
    /// own. Only the two refusals print, and both are red.
    pub(crate) fn turn_in_results(
        petition: &mut PetitionState,
        registrar: &mut GuildRegistrarState,
        result: u32,
    ) {
        if result == petition_result::OK {
            // The server has destroyed the charter, so the item-bound window has nothing left to
            // point at either.
            petition.close();
            registrar.close();
            return;
        }
        if let Some(line) = lines::turn_in_line(result) {
            petition.lines.push(line);
        }
    }

    /// `MSG_PETITION_DECLINE` inbound — somebody turned our charter down.
    ///
    /// **Only if their name is already cached** (`0x5ef12a`/`0x5ef139`). There is no query and no
    /// retry: an uncached name means the owner is told nothing at all, ever. So this arm
    /// deliberately does *not* take the command channel — it must not be able to ask.
    pub(crate) fn declined(petition: &mut PetitionState, names: &NameCache, player: u64) {
        if let Some(name) = names.peek(player) {
            let line = lines::declined_line(name);
            petition.lines.push(line);
        }
    }

    /// `MSG_PETITION_RENAME` inbound — the echo of a rename that took.
    ///
    /// Patches the cached title **in place** (`0x5ef292` overwrites the record's inline `char[0x100]`
    /// and fires `PETITION_SHOW`), rather than re-querying: the echo carries the new name.
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

    /// The open charter's cached title — what the feed reads to fill `GetPetitionInfo`'s second
    /// return, spelled once here rather than at three assertion sites.
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

    /// **The registrar's three gates**, and the price is entry\[0\]'s — not the first *visible*
    /// row's, which is what this file shipped first.
    ///
    /// The two readings agree on every packet vmangos sends (one row, flag set), which is exactly
    /// why the wrong one survives: it takes a two-row fixture whose first row is hidden to tell
    /// them apart, and the real client would drop that packet where a first-visible reading quotes
    /// the second row's price.
    #[test]
    fn the_registrar_gates_on_entry_zero_and_on_both_npc_flags() {
        let both = Some(REGISTRAR_NPC_FLAGS);

        let mut r = GuildRegistrarState::default();
        assert!(r.open(&show_list(0x2a1f, &[(1000, 1)]), both));
        assert_eq!(r.cost(), 1000, "entry[0]'s price, in copper");
        assert_eq!(r.npc(), Some(0x2a1f));

        // Entry[0] hidden: the WHOLE packet is dropped. A "first visible row" reading would open
        // the window here and quote 1000.
        let mut r = GuildRegistrarState::default();
        assert!(!r.open(&show_list(0x2a1f, &[(9999, 0), (1000, 1)]), both));
        assert_eq!(r.npc(), None, "no window at all — entry[0] is not visible");

        // Each NPC-flag gate alone is not enough; the client tests both, separately.
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

    /// The registrar is an NPC session and the petition window is not: closing the first must not
    /// touch the second. This is the whole reason they are two resources.
    #[test]
    fn walking_away_from_the_registrar_leaves_an_open_charter_alone() {
        let (commands, _rx) = commands();
        let mut registrar = GuildRegistrarState::default();
        let mut petition = PetitionState::default();
        registrar.open(&show_list(0x2a1f, &[(1000, 1)]), Some(REGISTRAR_NPC_FLAGS));
        petition.show(signatures(0x99, 0xaa, 7, &[0xbb]), &commands);

        registrar.close();
        assert_eq!(registrar.npc(), None);
        assert_eq!(
            petition.open_item(),
            Some(0x99),
            "the charter window is item-bound and survives the walk-away"
        );
    }

    /// The record query fires once per petition id and not again — the guild-identity cache's
    /// ask-once rule, which is what stops a window that repaints every frame from hammering the
    /// server.
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

    /// A rename echo patches the cached record in place. Re-querying instead would be a round trip
    /// for a value the echo already carries — and would leave the window showing the old name for
    /// as long as it took.
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

    /// **The charter window and the in-flight sign die with the session.** Only the Lua close,
    /// a sign result or a turn-in cleared `open`, and a logout's fresh VM never runs the old
    /// one's `OnHide` — so the next login's feed saw `None → Some` and fired `PETITION_SHOW` into
    /// the new character's UI, and closing that ghost put `MSG_PETITION_DECLINE` on the wire for
    /// the old session's charter. Driven through the real registration.
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
}
