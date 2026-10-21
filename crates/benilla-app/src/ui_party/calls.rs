//! The party and raid verbs the host applies inside their calls ([`PartyCalls`]): each verb's
//! packet goes out before it returns, as the reference's do (`InviteByName` at `0x48a4c8`,
//! `UninviteByGuid 0x5e9510`, `SetRaidTarget`'s send at `0x4bc01b`-`0x4bc03a`), so a chunk's
//! packets leave in call order whatever system each verb belongs to. The bindings have run their
//! own checks and resolves ([`benilla_ui::script::PartyRequest`]).
//!
//! In the `/partytest` sandbox, where no server holds the group, the verbs that change it are
//! answered by the drain as the server's echo would be ([`GroupEdits`], [`super::drain_party`]).

use benilla_protocol::messages::ObjectType;
use benilla_ui::script::{PartyRequest, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::feed::{
    cached_player_name, raid_guid_at, raid_row_subgroup, raid_target_mark, uninvite_by_guid,
    RaidTargetMark,
};
use super::GroupState;
use crate::names::NameCache;
use crate::net::{ClientCommand, GuidIndex, NetCommands, ObjectStore};
use crate::query_cache::Ask;
use crate::ui_action::{UiError, UiErrorKeys};

/// What a call leaves for the drain: the sandbox's echoes.
#[derive(Resource, Default)]
pub(crate) struct GroupEdits {
    /// The sandbox's group-changing calls, for the drain to answer as the server's echo would.
    pub(super) sandbox: Vec<SandboxEcho>,
}

/// One sandboxed call for the drain to echo.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SandboxEcho {
    /// A call the server answers with a fresh list ([`super::feed::test_apply_local`]).
    Request(PartyRequest),
    /// A mark's `MSG_RAID_TARGET_UPDATE`.
    RaidTarget { icon: u8, guid: u64 },
}

/// The engine half of the party and raid calls ([`crate::script_calls::ScriptCallHost`]).
#[derive(SystemParam)]
pub(crate) struct PartyCalls<'w, 's> {
    group: Res<'w, GroupState>,
    edits: ResMut<'w, GroupEdits>,
    names: Res<'w, NameCache>,
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
    commands: Res<'w, NetCommands>,
    errors: Res<'w, UiErrorKeys>,
    relations: crate::target::RelationWorld<'w, 's>,
    /// `InviteByGuid`'s acknowledge (`0x5e949c`); a world with no tutorial state has none.
    tutorials: Option<Res<'w, crate::tutorial::Tutorials>>,
    /// The lines a verb adds in place ([`PartyRequest::SystemLine`]).
    chat: Res<'w, crate::ui_chat::ChatLog>,
}

/// The calls that change the group, which the sandbox answers as the server would
/// ([`super::feed::test_apply_local`]); the rest stay real there: the invites, their answers,
/// the marks and the ready-check answer.
fn sandboxed(request: &PartyRequest) -> bool {
    matches!(
        request,
        PartyRequest::Leave
            | PartyRequest::UninviteGuid(_)
            | PartyRequest::LootMethod { .. }
            | PartyRequest::LootThreshold(_)
            | PartyRequest::ConvertToRaid
            | PartyRequest::SetSubgroup { .. }
            | PartyRequest::SwapSubgroup { .. }
            | PartyRequest::UninviteRaid(_)
            | PartyRequest::PromoteGuid(_)
            | PartyRequest::AssistantLeader { .. }
            | PartyRequest::ReadyCheckStart
            | PartyRequest::RequestRaidInfo
    )
}

impl PartyCalls<'_, '_> {
    /// `request` inside its verb: its packet, sent before the verb returns.
    pub(crate) fn apply(&mut self, script: &mut UiScript, request: PartyRequest) {
        if self.group.test && sandboxed(&request) {
            self.edits.sandbox.push(SandboxEcho::Request(request));
            return;
        }
        let held = |guid: u64| {
            self.index
                .0
                .get(&guid)
                .and_then(|e| self.stores.get(*e).ok())
        };
        let send = |command: ClientCommand| {
            let _ = self.commands.0.send(command);
        };
        match request {
            // `SendGroupAccept 0x5e96c0` and `SendGroupDecline 0x5e9720`: the packet and nothing
            // else, so the invite's popup stays until it hides itself.
            PartyRequest::Accept => send(ClientCommand::GroupAccept),
            PartyRequest::Decline => send(ClientCommand::GroupDecline),
            PartyRequest::Leave => send(ClientCommand::GroupLeave),
            // The binding `InviteByName` sends with no acknowledge (`0x48a495`-`0x48a4c8`).
            PartyRequest::InviteName(name) => send(ClientCommand::GroupInvite { name }),
            // The binding resolved the token (`0x48a3ed`). `InviteByGuid` (`0x5e9440`) invites
            // only a player the object manager holds (`0x5e945d`, typemask 0x10), by its
            // `GetUnitName` (`0x609210`), through `InviteByName` (`0x5e9477`), whose first act
            // acknowledges the grouping tutorial (`0x5e949c`), its flag sent ahead of the invite.
            PartyRequest::InviteGuid(guid) => {
                let Some(store) =
                    held(guid).filter(|s| matches!(s.0.object_type(), Some(ObjectType::Player)))
                else {
                    return;
                };
                let name = self
                    .names
                    .resolve_unit(guid, Some(store), Ask::Event(&self.commands))
                    .map_or_else(|| script.unknown_object_text(), str::to_string);
                if let Some(tutorials) = &self.tutorials {
                    tutorials.acknowledge_now(crate::tutorial::id::GROUPING, &self.commands);
                }
                send(ClientCommand::GroupInvite { name });
            }
            // The binding resolved the token (`0x48a54d`).
            PartyRequest::UninviteGuid(guid) => {
                uninvite_by_guid(guid, held(guid), &self.names, &self.commands, script);
            }
            // The binding resolved the method, the master and the threshold (`0x4e92a0`).
            PartyRequest::LootMethod {
                method,
                master,
                threshold,
            } => send(ClientCommand::LootMethod {
                method,
                master,
                threshold,
            }),
            // The binding resolved the token (`0x4bbeff`). A mark takes a raid row or its pet
            // (`0x4bafd0`), a party slot or its pet (`0x4e8fa0`), or, with the player's object
            // and the unit's held (`0x4bbf85`, `0x4bbfa4`), a unit that is no player
            // (`0x4bbfc1`), a player we can cooperate with (`0x606ba0`) or ourselves
            // (`0x4bbfcd`).
            PartyRequest::SetRaidTarget { guid, index } => {
                let relations = &self.relations;
                let markable = || {
                    let live = |g: u64| held(g).map(|s| &s.0);
                    self.group.member_or_pet(guid, None, live)
                        || relations
                            .me()
                            .zip(relations.unit(guid))
                            .is_some_and(|(me, unit)| {
                                !unit.is_player
                                    || relations.get().can_cooperate(&me, &unit)
                                    || unit.guid == me.guid
                            })
                };
                match raid_target_mark(&self.group, guid, index, markable) {
                    RaidTargetMark::Invalid => {
                        self.errors.push(UiError::key("ERR_INVALID_RAID_TARGET"));
                    }
                    RaidTargetMark::Nothing => {}
                    RaidTargetMark::Send { icon, guid } if self.group.test => {
                        self.edits
                            .sandbox
                            .push(SandboxEcho::RaidTarget { icon, guid });
                    }
                    RaidTargetMark::Send { icon, guid } => {
                        send(ClientCommand::SetRaidTarget { icon, guid });
                    }
                }
            }
            // The stored method and looter with the new threshold (`0x4e95aa`-`0x4e95b8`).
            PartyRequest::LootThreshold(threshold) => send(ClientCommand::LootMethod {
                method: u32::from(self.group.loot_stored.method),
                master: self.group.loot_stored.master,
                threshold,
            }),
            // ── The raid-management verbs ────────────────────────────────────────────────
            //
            // The RaidFrame names rows by index, UnitPopup by name. A row that is not there sends
            // nothing, as in the reference's bindings (`UninviteFromRaid`, `0x48a5c9`).
            PartyRequest::ConvertToRaid => send(ClientCommand::GroupRaidConvert),
            PartyRequest::SetSubgroup { index, group: sub } => {
                // Lua subgroups 1..8 are wire 0..7, the flags' bits 0-2
                // (`Server/Packets/Group.cpp:158`); out-of-range ones are dropped, never wrapped.
                let Some(sub) = (1..=8).contains(&sub).then(|| (sub - 1) as u8) else {
                    return;
                };
                // `0x4bb990`: a row (`0x4bba1f`-`0x4bba36`) not already in the subgroup
                // (`0x4bba3c`) whose name the cache holds (`0x4bba68`).
                let Some(guid) = raid_guid_at(&self.group, index) else {
                    return;
                };
                if raid_row_subgroup(&self.group, index) == Some(sub) {
                    return;
                }
                let Some(name) = cached_player_name(&self.names, guid) else {
                    return;
                };
                send(ClientCommand::GroupChangeSubGroup { name, group: sub });
            }
            PartyRequest::SwapSubgroup { index, other } => {
                // `0x4bbb00`: two rows (`0x4bbb71`-`0x4bbb99`) in different subgroups
                // (`0x4bbba5`) whose names the cache both holds (`0x4bbbec`-`0x4bbbf7`).
                let (Some(a), Some(b)) = (
                    raid_guid_at(&self.group, index),
                    raid_guid_at(&self.group, other),
                ) else {
                    return;
                };
                if raid_row_subgroup(&self.group, index) == raid_row_subgroup(&self.group, other) {
                    return;
                }
                let (Some(name), Some(other)) = (
                    cached_player_name(&self.names, a),
                    cached_player_name(&self.names, b),
                ) else {
                    return;
                };
                send(ClientCommand::GroupSwapSubGroup { name, other });
            }
            // The binding resolved the token or the name (`0x48a7fd`, `0x48a86a`, `0x4bbd6b`,
            // `0x4bbe3b`).
            PartyRequest::PromoteGuid(guid) => send(ClientCommand::GroupSetLeader { guid }),
            PartyRequest::AssistantLeader { guid, grant } => {
                send(ClientCommand::GroupAssistantLeader { guid, grant });
            }
            PartyRequest::UninviteRaid(row) => {
                if let Some(guid) = raid_guid_at(&self.group, row) {
                    uninvite_by_guid(guid, held(guid), &self.names, &self.commands, script);
                }
            }
            PartyRequest::ReadyCheckStart => send(ClientCommand::ReadyCheckStart),
            PartyRequest::SystemLine(text) => self.chat.show_now(
                script,
                &crate::ui_chat::ChatEvent::text_only(crate::ui_chat::ChatEventKind::System, text),
            ),
            PartyRequest::ReadyCheckAnswer(status) => {
                send(ClientCommand::ReadyCheckAnswer { status });
            }
            PartyRequest::RequestRaidInfo => send(ClientCommand::RequestRaidInfo),
        }
    }
}
