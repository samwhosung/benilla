//! The friend, ignore and `/who` getters' questions of the friend list (`0xc28168`) and the `/who`
//! array (`0xc281b8`), answered through the lent host ([`crate::script::live::ask`]) as of the
//! place the getter runs: the reference's getters read the list's slots and the array at the call
//! (`GetFriendInfo` `0x5ad060` through `0x5ae4b0`, `GetIgnoreName` `0x5ad460` through `0x5ae570`,
//! `GetWhoInfo` `0x5ad6e0` off `0xc2a120`). With no host free to ask, the getters read the pushed
//! snapshot ([`super::SocialState`]).

use super::{FriendInfo, WhoInfo};

/// What a friend, ignore or `/who` getter asks. An index is the binding's `_ftol` of its
/// argument, still 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocialRead {
    /// `GetNumFriends` (`0x5ad000` over `0x5ae490`): the slots before the first empty one.
    FriendCount,
    /// `GetFriendInfo(index)`: the slot `0x5ae4b0` finds for `index − 1`, up to 50 inclusive.
    Friend(u32),
    /// `GetSelectedFriend` (`0x5ad260`): `0x5ae510` plus one.
    SelectedFriend,
    /// `GetNumIgnores` (`0x5ad400` over `0x5ae550`): the ignore guids before the first empty one.
    IgnoreCount,
    /// `GetIgnoreName(index)`: the guid `0x5ae570` finds for `index − 1`, up to 25 inclusive.
    IgnoreGuid(u32),
    /// `GetSelectedIgnore`: `0x5ae630` plus one.
    SelectedIgnore,
    /// `GetNumWhoResults` (`0x5ad690`): the kept count `0xc2a120` and the total `0xc2a124`.
    WhoCounts,
    /// `GetWhoInfo(index)`: the array's slot `index − 1`, up to the kept count inclusive.
    Who(u32),
}

/// A [`SocialRead`]'s answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SocialAnswer {
    /// [`SocialRead::FriendCount`] and [`SocialRead::IgnoreCount`].
    Count(usize),
    /// [`SocialRead::Friend`]; `None` past slot 51, where the getter answers the `UNKNOWN` tuple
    /// (`0x5ad1c8`).
    Friend(Option<FriendInfo>),
    /// [`SocialRead::SelectedFriend`] and [`SocialRead::SelectedIgnore`]: 1-based, 0 for none.
    Index(u32),
    /// [`SocialRead::IgnoreGuid`]: 0 past slot 26 and for an empty slot (`0x5ae57b`).
    Guid(u64),
    /// [`SocialRead::WhoCounts`].
    WhoCounts { kept: u32, total: u32 },
    /// [`SocialRead::Who`]; `None` past the kept count's own slot, and after a full 50.
    Who(Option<WhoInfo>),
}

impl crate::script::live::LiveQuery for SocialRead {
    type Answer = SocialAnswer;
}

/// `read` through the lent host; `None` with no host free to ask, where the snapshot answers.
pub(crate) fn read_live(lua: &mlua::Lua, read: SocialRead) -> Option<SocialAnswer> {
    crate::script::live::ask(lua, &read)
}
