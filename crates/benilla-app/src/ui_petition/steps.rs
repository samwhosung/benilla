//! The charter verbs inside their calls ([`PetitionSteps`]): each sends its packet, shows its line
//! or closes the window before the verb returns, as the reference's bindings do (`0x4f46d0`,
//! `0x4f4790`, `0x4f4930`, `0x4f5050`, `0x4f5300`, `0x4f3f60`, `0x5ef2b0`).

use benilla_ui::script::{CallTail, PetitionCall, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{feed, lines, GuildRegistrarState, PetitionRecords, PetitionState};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfGuid, SelfPlayer};
use crate::query_cache::Ask;
use crate::ui_items::{find_item, ItemSearch};

/// The open charter, what its verbs read and what they send and show through: the engine half of
/// the charter calls ([`crate::script_calls::ScriptCallHost`]).
#[derive(SystemParam)]
pub(crate) struct PetitionSteps<'w, 's> {
    /// Absent, with the registrar and the records, in a world with no charter windows, which
    /// drops the calls.
    petition: Option<ResMut<'w, PetitionState>>,
    records: Option<Res<'w, PetitionRecords>>,
    registrar: Option<ResMut<'w, GuildRegistrarState>>,
    /// The designer `GetTabardInfo`'s opener tests (`0x5e00e6`).
    tabard: Option<Res<'w, crate::ui_tabard::TabardOpen>>,
    names: Res<'w, NameCache>,
    objects: Objects<'w, 's>,
    self_q: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    self_guid: Option<Res<'w, SelfGuid>>,
    /// `OfferPetition`'s target: `CGGameUI`'s selection pair (`0xb4e2d8`), not its interaction pair.
    selection: Res<'w, crate::target::Selection>,
    net: Res<'w, NetCommands>,
    sink: crate::ui_action::MessageSink<'w>,
}

impl PetitionSteps<'_, '_> {
    /// The registrar's cell `[0xbdceb0]`, which its close core tests (`0x4f5010`-`0x4f501b`).
    pub(crate) fn registrar_npc(&self) -> Option<u64> {
        self.registrar.as_deref().and_then(GuildRegistrarState::npc)
    }

    /// The close core's clear of the cell (`0x4f502b`-`0x4f5030`), after its release.
    pub(crate) fn clear_registrar(&mut self) {
        if let Some(registrar) = self.registrar.as_deref_mut() {
            registrar.clear();
        }
    }

    /// Apply `call` inside its verb; `ClosePetition`'s record drop is the tail, after the held
    /// `PETITION_CLOSED` handlers.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: PetitionCall) -> Option<CallTail> {
        if self.petition.is_none() || self.records.is_none() || self.registrar.is_none() {
            debug!("ui_petition: no charter state in this world; {call:?} dropped");
            return None;
        }
        let me = self.self_guid.as_ref().and_then(|g| g.0).unwrap_or(0);
        let registrar_npc = self.registrar.as_deref().and_then(GuildRegistrarState::npc);
        let send = |net: &NetCommands, command| {
            let _ = net.0.send(command);
        };
        match call {
            // The purchase `0x4f5050`: the petition module's latch (`[0xbdceb0]`), not the target
            // or `CGGameUI`'s NPC.
            PetitionCall::Buy(name) => {
                let Some(npc) = registrar_npc else {
                    debug!("ui_petition: BuyGuildCharter with no registrar open — dropped");
                    return None;
                };
                send(&self.net, ClientCommand::PetitionBuy { npc, name });
            }
            // The name check's own line (`0x4f5160` → `0x496720`); no packet is built.
            PetitionCall::NameRefused(key) => self.show(script, lines::name_refused_line(key)),
            // A fresh bag scan per call (`0x5ef2b0`); the charter's `max_count` is 1. No charter,
            // no packet: a red line (`0x5ef49a`).
            PetitionCall::TurnIn => {
                let charter = self.self_q.iter().next().and_then(|store| {
                    find_item(
                        &store.0,
                        &self.objects,
                        benilla_protocol::messages::CHARTER_ITEM_ENTRY,
                        ItemSearch::default(),
                    )
                });
                match charter {
                    Some((_, _, item)) => {
                        send(&self.net, ClientCommand::TurnInPetition { item });
                    }
                    None => self.show(script, lines::no_charter_line()),
                }
            }
            // The opener inside the call; the host runs the registrar's close core after it.
            PetitionCall::TabardInfo => {
                let designer = self.tabard.as_ref().and_then(|t| t.npc()).unwrap_or(0);
                let refusal = self.self_q.iter().next().and_then(|store| {
                    tabard_vendor_activate(registrar_npc.unwrap_or(0), designer, store, &self.net)
                });
                if let Some(key) = refusal {
                    self.show(script, crate::ui_action::UiError::key(key));
                }
            }
            // `0x4f46d0`: nothing without the item's guid (`0x4f470d`); then the send and the
            // in-flight latch, so a close while our signature is outstanding is no decline.
            PetitionCall::Sign(byte) => {
                let petition = self.petition.as_deref_mut().expect("checked above");
                let item = petition.open_item()?;
                send(&self.net, ClientCommand::PetitionSign { item, byte });
                petition.signing = true;
            }
            PetitionCall::Offer => self.offer(script, me),
            // `0x4f4930`: the open item's guid first (`0x4f495e`), and with none nothing at all
            // (`0x4f4970`); then the name check's line or the send (`0x4f4987`-`0x4f49d5`).
            PetitionCall::Rename { name, refusal } => {
                let item = self
                    .petition
                    .as_deref()
                    .and_then(PetitionState::open_item)?;
                match refusal {
                    Some(key) => self.show(script, lines::name_refused_line(key)),
                    None => send(&self.net, ClientCommand::PetitionRename { item, name }),
                }
            }
            // The registrar's close core needs the latch: the host runs it
            // ([`crate::script_calls::ScriptCallHost`]).
            PetitionCall::CloseRegistrar => {}
            PetitionCall::Close => {
                let petition = self.petition.as_deref_mut().expect("checked above");
                petition.open_item()?; // `0x4f3f7c`: nothing open, no event
                let records = self.records.as_deref().expect("checked above");
                petition.decline_on_close(records, me, &self.net);
                petition.close();
                petition.fed.closed();
                // `0x4f3ef0` frees the signers, then `PETITION_CLOSED` (`0x4f402c`), whose
                // handlers still read the record, which goes after them (`0x4f4078`).
                script.free_petition_signers();
                script.fire_event("PETITION_CLOSED", vec![]);
                return Some(Box::new(|script: &mut UiScript| {
                    script.drop_petition_record()
                }));
            }
        }
        None
    }

    /// `OfferPetition()` (`0x4f4790`): nothing without a petition (`0x4f47ac`) or a target
    /// (`0x4f47bd`); the player itself shows `ERR_PETITION_CREATOR` (`0x4f4839`); then the send and
    /// `ERR_PETITION_OFFERED_S` at once, with no reply awaited (`0x4f48dc`-`0x4f48fa`). Guards 3-5,
    /// 7 and 8 are not built, and the server refuses those.
    fn offer(&mut self, script: &mut UiScript, me: u64) {
        let Some(item) = self.petition.as_deref().and_then(PetitionState::open_item) else {
            return;
        };
        let Some(player) = self.selection.guid else {
            debug!("ui_petition: OfferPetition with no target — dropped");
            return;
        };
        if player == me {
            return self.show(script, lines::self_offer_line());
        }
        let _ = self
            .net
            .0
            .send(ClientCommand::OfferPetition { item, player });
        let name = self
            .names
            .resolve_player(player, Ask::Event(&self.net))
            .unwrap_or_default()
            .to_string();
        self.show(script, lines::offered_line(&name));
    }

    /// Show `line` inside the call, as `0x496720` does; a key the VM's `GlobalStrings.lua` lacks
    /// shows nothing.
    fn show(&mut self, script: &mut UiScript, line: lines::Line) {
        if let Some(shown) = feed::shown(script, &line) {
            crate::ui_action::show_now(script, &mut self.sink, "ui_petition", shown);
        }
    }
}

/// `0x5e00e0`, the designer's opener, for `GetTabardInfo` with the registrar's guid `npc`
/// (`0x4f5338`): nothing while the designer is open on that guid (`0x5e00e6`-`0x5e00fc`, so two
/// zero guids send nothing), the refusal's key while the player's `UNIT_FIELD_DISPLAYID` is not its
/// `UNIT_FIELD_NATIVEDISPLAYID` (`0x5e0102`-`0x5e0116`, error `0x130`), else the packet.
fn tabard_vendor_activate(
    npc: u64,
    designer: u64,
    player: &ObjectStore,
    commands: &NetCommands,
) -> Option<&'static str> {
    if designer == npc {
        return None;
    }
    if player.0.unit_displayid() != player.0.unit_native_displayid() {
        return Some("ERR_EMBLEMERROR_NOTABARDGEOSET");
    }
    let _ = commands.0.send(ClientCommand::TabardVendorActivate { npc });
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::ObjectFields;

    /// The player's display ids, `UNIT_FIELD_DISPLAYID` (131) and `UNIT_FIELD_NATIVEDISPLAYID`.
    fn player(display: u32, native: u32) -> ObjectStore {
        ObjectStore(ObjectFields::from_pairs(&[(131, display), (132, native)]))
    }

    fn run(
        npc: u64,
        designer: u64,
        me: &ObjectStore,
    ) -> (Vec<ClientCommand>, Option<&'static str>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let refusal = tabard_vendor_activate(npc, designer, me, &NetCommands(tx));
        (rx.try_iter().collect(), refusal)
    }

    #[test]
    fn get_tabard_info_opens_the_designer_for_the_registrar() {
        let (sent, refusal) = run(0x42, 0, &player(49, 49));
        assert!(
            matches!(
                sent.as_slice(),
                [ClientCommand::TabardVendorActivate { npc: 0x42 }]
            ),
            "{sent:?}"
        );
        assert_eq!(refusal, None);
    }

    #[test]
    fn get_tabard_info_sends_nothing_to_a_designer_already_open_on_that_npc() {
        let (sent, refusal) = run(0x42, 0x42, &player(49, 49));
        assert!(sent.is_empty() && refusal.is_none());
        // With no registrar and no designer both guids are 0: equal, so nothing.
        let (sent, _) = run(0, 0, &player(49, 49));
        assert!(sent.is_empty());
    }

    #[test]
    fn get_tabard_info_refuses_a_shapeshifted_player() {
        let (sent, refusal) = run(0x42, 0, &player(2281, 49));
        assert!(sent.is_empty());
        assert_eq!(refusal, Some("ERR_EMBLEMERROR_NOTABARDGEOSET"));
    }
}
