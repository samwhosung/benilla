//! The charter getters' reads at the call, the call host's half of
//! [`benilla_ui::script::PetitionQuery`]: the registrar's charter cost (`0xbdce50`, `0x4f5040`),
//! the signer list (`0xbdce20`) with each name read off the name cache (`0x4f45a1`), the cached
//! record the window holds (`0xbdce28`, `0x4f43d0`) with its owner's name (`0x4f4459`), and
//! `CanSignPetition` (`0x4f45e0`) over them, as the handlers and the calls left them.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{
    PetitionQuery, PetitionRecordView, PetitionState as VmPetition, PETITION_TYPE_CHARTER,
    PETITION_TYPE_PETITION,
};

use super::{GuildRegistrarState, PetitionRecords, PetitionState};
use crate::names::NameCache;
use crate::net::{Objects, SelfGuid};
use crate::query_cache::Ask;

/// What the charter getters read as they run; each absent in a harness without it, where the
/// getters read the VM's copy.
#[derive(SystemParam)]
pub(crate) struct PetitionLive<'w, 's> {
    petition: Option<Res<'w, PetitionState>>,
    records: Option<Res<'w, PetitionRecords>>,
    registrar: Option<Res<'w, GuildRegistrarState>>,
    names: Option<Res<'w, NameCache>>,
    me: Option<Res<'w, SelfGuid>>,
    objects: Objects<'w, 's>,
}

impl PetitionLive<'_, '_> {
    /// The charter state now; every name only read.
    pub(super) fn state(&self) -> Option<VmPetition> {
        let (petition, records, registrar, names) = (
            self.petition.as_deref()?,
            self.records.as_deref()?,
            self.registrar.as_deref()?,
            self.names.as_deref()?,
        );
        let me = self.me.as_deref().and_then(|g| g.0).unwrap_or(0);
        let signers: Vec<Option<String>> = petition
            .signers
            .iter()
            .map(|&s| names.resolve_player(s, Ask::Read).map(str::to_string))
            .collect();
        let record = petition
            .record
            .and_then(|id| records.records.get(id))
            .map(|r| PetitionRecordView {
                petition_type: if r.is_charter {
                    PETITION_TYPE_CHARTER
                } else {
                    PETITION_TYPE_PETITION
                }
                .to_string(),
                title: r.name.clone(),
                body_text: r.body_text.clone(),
                max_signatures: r.required,
                originator: names.resolve_player(r.owner, Ask::Read).map(str::to_string),
                // The record's owner against the active player's guid (`0x4f447a`).
                is_originator: r.owner == me,
            });
        // `0x4f45e0`: with a record, a charter refuses a guilded player (`0x4f4621`) or a full list
        // (`0x4f4634`), and the owner (`0x4f4666`); with none it jumps to the signer scan
        // (`0x4f45f7`), which refuses a listed player either way.
        let in_guild = self
            .objects
            .player_fields()
            .is_some_and(|p| p.player_guild_id() != 0);
        let refused = record.as_ref().is_some_and(|r| {
            (r.petition_type == PETITION_TYPE_CHARTER
                && (in_guild || petition.signers.len() as i32 >= r.max_signatures))
                || r.is_originator
        });
        let can_sign = !refused && !petition.signers.contains(&me);
        Some(VmPetition {
            charter_cost: registrar.cost(),
            signers,
            record,
            can_sign,
        })
    }
}

impl crate::script_calls::live::LiveFamily for PetitionLive<'_, '_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|_: &PetitionQuery| self.state())
    }
}
