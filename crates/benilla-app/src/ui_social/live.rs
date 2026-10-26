//! The friend, ignore and `/who` getters' questions at the call, the call host's half of
//! [`benilla_ui::script::SocialRead`]: the friend list's slots, its selections and the `/who`
//! array as the handlers and the verbs left them, each row resolved as `GetFriendInfo`
//! (`0x5ad060`) and `GetWhoInfo` (`0x5ad6e0`) resolve theirs. An ignore's name is the names
//! family's read ([`crate::names::NameLive`]).

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use benilla_ui::script::live::LiveAsk;
use benilla_ui::script::{SocialAnswer, SocialRead};

use super::{feed, SocialState, FRIEND_SLOTS, IGNORE_SLOTS};
use crate::area::AreaTableRes;
use crate::ui_unit::NameTables;

/// What the friend, ignore and `/who` getters read as they run.
#[derive(SystemParam)]
pub(crate) struct SocialLive<'w> {
    social: Option<Res<'w, SocialState>>,
    areas: Option<Res<'w, AreaTableRes>>,
    tables: NameTables<'w>,
}

impl SocialLive<'_> {
    fn read(&self, social: &SocialState, read: SocialRead) -> SocialAnswer {
        let areas = self.areas.as_deref().map(|a| &a.0);
        let names = self.tables.get();
        let listed = |guids: &mut dyn Iterator<Item = u64>| guids.take_while(|&g| g != 0).count();
        match read {
            // `0x5ae490`: the guids from `+8` in steps of `0x20`, to the first 0.
            SocialRead::FriendCount => SocialAnswer::Count(listed(
                &mut social.slots.iter().take(FRIEND_SLOTS).map(|s| s.guid),
            )),
            // `0x5ae4b0` on `index − 1`, compared unsigned: slot 50 lies on `+0x640`.
            SocialRead::Friend(index) => {
                let slot = index.wrapping_sub(1) as usize;
                SocialAnswer::Friend(match social.slots.get(slot) {
                    Some(s) => Some(feed::friend_row(s, areas, names)),
                    None if slot == FRIEND_SLOTS => Some(feed::friend_row(
                        &feed::after_the_slots(social),
                        areas,
                        names,
                    )),
                    None => None,
                })
            }
            SocialRead::SelectedFriend => SocialAnswer::Index(social.selected_friend_index()),
            // `0x5ae550`: the guids from `+0x650` in steps of 8, to the first 0.
            SocialRead::IgnoreCount => SocialAnswer::Count(listed(
                &mut social.ignores.iter().take(IGNORE_SLOTS).copied(),
            )),
            // `0x5ae570`: `cmp edx,0x19; jbe` (`0x5ae576`), slot 25 the pending count at `+0x718`
            // and the dword after it, which nothing writes, held 0.
            SocialRead::IgnoreGuid(index) => {
                let slot = index.wrapping_sub(1) as usize;
                SocialAnswer::Guid(match social.ignores.get(slot) {
                    Some(&guid) => guid,
                    None if slot == IGNORE_SLOTS => u64::from(social.ignores_pending),
                    None => 0,
                })
            }
            SocialRead::SelectedIgnore => SocialAnswer::Index(social.selected_ignore_index()),
            SocialRead::WhoCounts => SocialAnswer::WhoCounts {
                kept: social.who_count,
                total: social.who_total,
            },
            // `0x5ad71b` `cmp eax,[0xc2a120]; ja`: the kept count's own slot passes, reading what
            // an earlier answer left there.
            SocialRead::Who(index) => {
                let slot = index.wrapping_sub(1) as usize;
                // Deviation: after a full 50 the reference reads the 160 bytes past the array
                // (`0xc2a0f8`, the counts at `0xc2a120` among them); benilla answers the miss.
                SocialAnswer::Who(
                    (slot <= social.who_count as usize)
                        .then(|| social.who_slots.get(slot))
                        .flatten()
                        .map(|e| feed::who_row(e, areas, names)),
                )
            }
        }
    }
}

impl crate::script_calls::live::LiveFamily for SocialLive<'_> {
    fn answer(&mut self, ask: &mut LiveAsk<'_>) -> bool {
        ask.answer(|q: &SocialRead| {
            let social = self.social.as_deref()?;
            Some(self.read(social, *q))
        })
    }
}
