//! The friend and ignore verbs inside their calls ([`SocialSteps`]): each sends its packet, moves
//! its selection or shows its line before the verb returns, as the reference's `FriendList`
//! functions do.

use benilla_ui::script::{SocialCall, UiScript};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use super::{SocialState, FRIEND_SLOTS, IGNORE_SLOTS};
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands};
use crate::query_cache::Ask;
use crate::ui_action::{UiError, UiErrorKeys};

/// The social state and what its verbs send through: the engine half of the social calls
/// ([`crate::script_calls::ScriptCallHost`]).
#[derive(SystemParam)]
pub(crate) struct SocialSteps<'w> {
    social: ResMut<'w, SocialState>,
    names: Res<'w, NameCache>,
    net: Res<'w, NetCommands>,
    errors: Res<'w, UiErrorKeys>,
}

impl SocialSteps<'_> {
    /// Apply `call` inside its verb, then seat the selections it leaves for the verb's next line.
    pub(crate) fn apply(&mut self, script: &mut UiScript, call: SocialCall) {
        match call {
            SocialCall::ShowFriends => self.send(ClientCommand::FriendListRequest),
            SocialCall::AddFriend(name) => self.add_friend(script, name),
            SocialCall::RemoveFriendIndex(index) => self.remove_friend_index(index),
            SocialCall::RemoveFriendName(name) => self.remove_friend_name(&name),
            SocialCall::SelectFriend(index) => self.select_friend(index),
            SocialCall::AddIgnore(name) => self.add_ignore(name),
            SocialCall::DelIgnore(name) => self.del_ignore(&name),
            SocialCall::AddOrDelIgnore(name) => self.add_or_del_ignore(name),
            SocialCall::SelectIgnore(index) => self.select_ignore(index),
        }
        script.set_social_selection(
            self.social.selected_friend_index(),
            self.social.selected_ignore_index(),
        );
    }

    fn send(&self, command: ClientCommand) {
        let _ = self.net.0.send(command);
    }

    /// `FriendList::AddFriend` (`0x5ae670`): tutorial 21 seen (`0x5ae677`), then `CMSG_ADD_FRIEND`
    /// with the name as given (`0x5ae6ae`).
    fn add_friend(&mut self, script: &mut UiScript, name: String) {
        script.queue_tutorial_acknowledge(crate::tutorial::id::FRIENDS);
        self.send(ClientCommand::AddFriend { name });
    }

    /// `RemoveFriendByIndex` (`0x5ae7e0`): `CMSG_DEL_FRIEND` for slot `index − 1` below 50 that
    /// holds a guid (`0x5ae7e6`, `0x5ae7fc`).
    fn remove_friend_index(&mut self, index: u32) {
        let slot = index.wrapping_sub(1) as usize;
        let guid = self.social.slots.get(slot).map_or(0, |s| s.guid);
        if slot < FRIEND_SLOTS && guid != 0 {
            self.send(ClientCommand::DelFriend { guid });
        }
    }

    /// `RemoveFriendByName` (`0x5ae6f0`): the first slot whose own name matches by `SStrCmpI`
    /// (`0x5ae710`), else `ERR_FRIEND_NOT_FOUND` (`0x5ae722`).
    fn remove_friend_name(&mut self, name: &str) {
        let hit = self.social.slots.iter().find(|s| {
            s.name
                .as_deref()
                .is_some_and(|n| benilla_ui::crt::str_cmp_i(n, name) == 0)
        });
        match hit.map(|s| s.guid) {
            Some(guid) => self.send(ClientCommand::DelFriend { guid }),
            None => self.errors.push(UiError::key("ERR_FRIEND_NOT_FOUND")),
        }
    }

    /// `FriendList::SetSelectedFriend` (`0x5ae4d0`): the guid of slot `index − 1` up to 50
    /// inclusive (`0x5ae4d6`), else none. Slot 50 is the selection's own field (`+0x648`), so it
    /// keeps the selection.
    fn select_friend(&mut self, index: u32) {
        let slot = index.wrapping_sub(1) as usize;
        match self.social.slots.get(slot) {
            Some(s) => self.social.selected_friend = s.guid,
            None if slot == FRIEND_SLOTS => {}
            None => self.social.selected_friend = 0,
        }
    }

    /// The first ignore slot whose guid's cached name matches `name` by `SStrCmpI`, as the ignore
    /// verbs scan the 25 slots with the lookup that never asks (`0x5af1ef`, `0x5af345`,
    /// `0x5af447`); an empty slot has no name.
    fn ignore_named(&self, name: &str) -> Option<(u64, String)> {
        self.social.ignores.iter().find_map(|&guid| {
            let cached = self.names.resolve_player(guid, Ask::Read)?;
            (benilla_ui::crt::str_cmp_i(cached, name) == 0).then(|| (guid, cached.to_string()))
        })
    }

    /// `FriendList::AddIgnore` (`0x5af300`): a listed match shows `ERR_IGNORE_ALREADY_S` with its
    /// cached name (`0x5af3d9`), else `CMSG_ADD_IGNORE` with the name.
    fn add_ignore(&mut self, name: String) {
        match self.ignore_named(&name) {
            Some((_, cached)) => self.errors.push(UiError::s("ERR_IGNORE_ALREADY_S", cached)),
            None => self.send(ClientCommand::AddIgnore { name }),
        }
    }

    /// `FriendList::DelIgnore` (`0x5af400`): `CMSG_DEL_IGNORE` for a listed match (`0x5af497`),
    /// else `ERR_IGNORE_NOT_FOUND` (`0x5af46c`).
    fn del_ignore(&mut self, name: &str) {
        match self.ignore_named(name) {
            Some((guid, _)) => self.send(ClientCommand::DelIgnore { guid }),
            None => self.errors.push(UiError::key("ERR_IGNORE_NOT_FOUND")),
        }
    }

    /// `FriendList::AddOrDelIgnore` (`0x5af1a0`): `CMSG_DEL_IGNORE` for a listed match
    /// (`0x5af27f`), else `CMSG_ADD_IGNORE` with the name (`0x5af21f`).
    fn add_or_del_ignore(&mut self, name: String) {
        match self.ignore_named(&name) {
            Some((guid, _)) => self.send(ClientCommand::DelIgnore { guid }),
            None => self.send(ClientCommand::AddIgnore { name }),
        }
    }

    /// `FriendList::SetSelectedIgnore` (`0x5ae5f0`): the guid of ignore slot `index − 1` up to 25
    /// inclusive (`0x5ae5f6`), else none. Slot 25 is the eight bytes at `+0x718`: the ignore
    /// pending count, then `+0x71c`, which nothing writes: the list's allocation is unzeroed
    /// (`SMemAlloc` flags 0 at `0x5adc60`) and its constructor `0x5aca10` skips it, so the
    /// reference reads whatever the heap held, and benilla holds 0.
    fn select_ignore(&mut self, index: u32) {
        let slot = index.wrapping_sub(1) as usize;
        self.social.selected_ignore = match self.social.ignores.get(slot) {
            Some(&guid) => guid,
            None if slot == IGNORE_SLOTS => u64::from(self.social.ignores_pending),
            None => 0,
        };
    }
}
