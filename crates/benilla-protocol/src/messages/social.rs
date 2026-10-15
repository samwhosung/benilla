//! The social family: friend list, ignore list and `/who` (0x62-0x6d). The 1.12 client holds 50
//! friends and 25 ignores (the `FriendList` slot arrays at `+0x008` and `+0x650`), the vmangos
//! limits too; FrameXML's `MAX_IGNORE = 50` is dead data.

use std::io::{self, Read};

use crate::wire::{capacity_hint, read_cstring, read_u32_le, read_u64_le, read_u8};

/// `SMSG_FRIEND_STATUS`'s result byte, vmangos `FriendsResult` (`SocialMgr.h:81-110`). The client
/// (`0x5acab0`) prints each result's `ERR_*` string to the chat frame, not the red error line.
pub mod friend_result {
    /// `ERR_FRIEND_DB_ERROR`: the server's lookup failed.
    pub const DB_ERROR: u8 = 0x00;
    /// `ERR_FRIEND_LIST_FULL`: 50 friends already.
    pub const LIST_FULL: u8 = 0x01;
    /// `ERR_FRIEND_ONLINE_SS`: a friend just logged in (broadcast, not an ack).
    pub const ONLINE: u8 = 0x02;
    /// `ERR_FRIEND_OFFLINE_S`: a friend just logged out (broadcast).
    pub const OFFLINE: u8 = 0x03;
    /// `ERR_FRIEND_NOT_FOUND`: no character by that name.
    pub const NOT_FOUND: u8 = 0x04;
    /// `ERR_FRIEND_REMOVED_S`: the ack for `CMSG_DEL_FRIEND`.
    pub const REMOVED: u8 = 0x05;
    /// `ERR_FRIEND_ADDED_S`, friend online: carries the online tail.
    pub const ADDED_ONLINE: u8 = 0x06;
    /// `ERR_FRIEND_ADDED_S`, friend offline: no tail.
    pub const ADDED_OFFLINE: u8 = 0x07;
    /// `ERR_FRIEND_ALREADY_S`.
    pub const ALREADY: u8 = 0x08;
    /// `ERR_FRIEND_SELF`.
    pub const SELF: u8 = 0x09;
    /// `ERR_FRIEND_WRONG_FACTION`: cross-faction friending, refused by config.
    pub const ENEMY: u8 = 0x0A;
    /// `ERR_IGNORE_FULL`: 25 ignores already.
    pub const IGNORE_FULL: u8 = 0x0B;
    /// `ERR_IGNORE_SELF`.
    pub const IGNORE_SELF: u8 = 0x0C;
    /// `ERR_IGNORE_NOT_FOUND`.
    pub const IGNORE_NOT_FOUND: u8 = 0x0D;
    /// `ERR_IGNORE_ALREADY_S`.
    pub const IGNORE_ALREADY: u8 = 0x0E;
    /// `ERR_IGNORE_ADDED_S`: the ack for `CMSG_ADD_IGNORE`.
    pub const IGNORE_ADDED: u8 = 0x0F;
    /// `ERR_IGNORE_REMOVED_S`: the ack for `CMSG_DEL_IGNORE`.
    pub const IGNORE_REMOVED: u8 = 0x10;
    /// `ERR_IGNORE_AMBIGUOUS`.
    pub const IGNORE_AMBIGUOUS: u8 = 0x11;
    /// `ERR_FRIEND_ERROR` ("Unknown friend response from server."), vmangos `FRIEND_UNKNOWN`.
    pub const UNKNOWN: u8 = 0x1A;
}

/// A friend's presence byte, vmangos `FriendStatus` (`SocialMgr.h:36-43`).
pub mod friend_status {
    /// Offline: the entry carries no area/level/class.
    pub const OFFLINE: u8 = 0;
    /// Online, no away flag.
    pub const ONLINE: u8 = 1;
    /// Online and flagged AFK.
    pub const AFK: u8 = 2;
    /// Online and flagged DND (`3` is unused by 1.12 vmangos).
    pub const DND: u8 = 4;
}

/// One friend-list row. The name is not on the wire: the client resolves it through its name cache
/// (`0x55f080`, filled by `CMSG_NAME_QUERY`) when it draws the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FriendEntry {
    /// Always a player guid: vmangos rebuilds it as `ObjectGuid(HIGHGUID_PLAYER, lowguid)`.
    pub guid: u64,
    /// A [`friend_status`]; when `OFFLINE`, the fields below were not sent and read as zero.
    pub status: u8,
    /// The friend's zone id (`AreaTable.dbc`).
    pub area: u32,
    pub level: u32,
    pub class: u32,
}

impl FriendEntry {
    /// Any of ONLINE/AFK/DND: the wire's own test for whether the area/level/class tail is sent.
    pub fn is_online(&self) -> bool {
        self.status != friend_status::OFFLINE
    }
}

/// Read `SMSG_FRIEND_LIST` (vmangos `Social.cpp`, client `0x5ae350`): a `u8` count, then per
/// friend a `u64` guid, a `u8` status and, only if the status is non-zero, `u32` area/level/class.
pub fn read_friend_list(r: &mut impl Read) -> io::Result<Vec<FriendEntry>> {
    let count = read_u8(r)?;
    // vmangos `SOCIALMGR_FRIEND_LIMIT` 50 (`SocialMgr.h:114`).
    let mut friends = Vec::with_capacity(capacity_hint(count, 50));
    for _ in 0..count {
        let mut entry = FriendEntry {
            guid: read_u64_le(r)?,
            status: read_u8(r)?,
            ..Default::default()
        };
        if entry.is_online() {
            entry.area = read_u32_le(r)?;
            entry.level = read_u32_le(r)?;
            entry.class = read_u32_le(r)?;
        }
        friends.push(entry);
    }
    Ok(friends)
}

/// Read `SMSG_IGNORE_LIST` (vmangos `Social.cpp`): a `u8` count, then that many guids.
pub fn read_ignore_list(r: &mut impl Read) -> io::Result<Vec<u64>> {
    let count = read_u8(r)?;
    (0..count).map(|_| read_u64_le(r)).collect()
}

/// `SMSG_FRIEND_STATUS`: one result about one player, with their whereabouts when online.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FriendStatusUpdate {
    /// A [`friend_result`] code, either an ack or a login/logout broadcast; the code says which.
    pub result: u8,
    /// The player the result is about; 0 for `NOT_FOUND`.
    pub guid: u64,
    /// Present only for [`friend_result::ONLINE`] and [`friend_result::ADDED_ONLINE`].
    pub online: Option<FriendOnline>,
}

/// The presence tail of an online `SMSG_FRIEND_STATUS`: the four fields a [`FriendEntry`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FriendOnline {
    /// A [`friend_status`] (the vmangos `SUPPORTED_CLIENT_BUILD > CLIENT_BUILD_1_8_4` arm).
    pub status: u8,
    /// Zone id.
    pub area: u32,
    pub level: u32,
    pub class: u32,
}

/// Read `SMSG_FRIEND_STATUS` (vmangos `SocialMgr::SendFriendStatus`, client `0x5add30`): `u8`
/// result, `u64` guid, then the online tail when the result code says so, whatever the length.
pub fn read_friend_status(r: &mut impl Read) -> io::Result<FriendStatusUpdate> {
    let result = read_u8(r)?;
    let guid = read_u64_le(r)?;
    let online = matches!(result, friend_result::ONLINE | friend_result::ADDED_ONLINE)
        .then(|| -> io::Result<FriendOnline> {
            Ok(FriendOnline {
                status: read_u8(r)?,
                area: read_u32_le(r)?,
                level: read_u32_le(r)?,
                class: read_u32_le(r)?,
            })
        })
        .transpose()?;
    Ok(FriendStatusUpdate {
        result,
        guid,
        online,
    })
}

/// One `/who` hit; unlike a friend row, every field is on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WhoEntry {
    pub name: String,
    /// An empty cstring when unguilded.
    pub guild: String,
    pub level: u32,
    pub class: u32,
    pub race: u32,
    /// `AreaTable.dbc` zone id; the client resolves the name.
    pub zone: u32,
}

/// `SMSG_WHO`: the results and the two counts of the "N players total (M displayed)" line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WhoResults {
    /// Entries that follow (`clientCount`); vmangos stops at 49.
    pub displayed: u32,
    /// The full match count when over 49, else equal to [`Self::displayed`].
    pub total: u32,
    /// In server order.
    pub entries: Vec<WhoEntry>,
}

/// Read `SMSG_WHO` (vmangos `WhoListClientQueryTask`, client `0x5adde0`): two `u32` counts, then
/// `{name, guild, level, class, race, zone}` per entry. 1.12 has no trailing party-status `u32`.
pub fn read_who(r: &mut impl Read) -> io::Result<WhoResults> {
    let displayed = read_u32_le(r)?;
    let total = read_u32_le(r)?;
    let mut entries = Vec::with_capacity(capacity_hint(displayed, 64));
    for _ in 0..displayed {
        entries.push(WhoEntry {
            name: read_cstring(r)?,
            guild: read_cstring(r)?,
            level: read_u32_le(r)?,
            class: read_u32_le(r)?,
            race: read_u32_le(r)?,
            zone: read_u32_le(r)?,
        });
    }
    Ok(WhoResults {
        displayed,
        total,
        entries,
    })
}

/// A `/who` query parsed from the typed filter, e.g. `z-"Elwynn Forest" 1-10`, which the 1.12
/// engine (not FrameXML) parses for `SendWho`. [`Default`] is a bare `/who`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhoRequest {
    /// Inclusive minimum level.
    pub level_min: u32,
    /// Inclusive maximum level; the 1.12 client sends 100 when unset, not 255.
    pub level_max: u32,
    /// `n-`: a name substring, empty for any.
    pub player_name: String,
    /// `g-`: a guild-name substring, empty for any.
    pub guild_name: String,
    /// `r-`: a bitmask over race ids (`1 << race`), all-ones for any.
    pub race_mask: u32,
    /// `c-`: a bitmask over class ids (`1 << class`), all-ones for any.
    pub class_mask: u32,
    /// `z-`: zone ids, resolved from names client-side.
    pub zones: Vec<u32>,
    /// Untagged words; the server matches each against name, guild and zone name.
    pub search_terms: Vec<String>,
}

impl Default for WhoRequest {
    fn default() -> Self {
        Self {
            level_min: 0,
            level_max: 100,
            player_name: String::new(),
            guild_name: String::new(),
            race_mask: u32::MAX,
            class_mask: u32::MAX,
            zones: Vec::new(),
            search_terms: Vec::new(),
        }
    }
}

/// Over 10 zones or 4 search terms, `HandleWhoOpcode` silently drops the whole query.
pub const WHO_MAX_ZONES: usize = 10;
/// See [`WHO_MAX_ZONES`].
pub const WHO_MAX_SEARCH_TERMS: usize = 4;

/// Body of `CMSG_WHO` (vmangos `Misc.cpp`, `Who::ReadFromWorldPacket`). Both arrays are trimmed to
/// the server's caps, so an over-long query narrows instead of matching nobody.
pub fn who(request: &WhoRequest) -> Vec<u8> {
    let zones = &request.zones[..request.zones.len().min(WHO_MAX_ZONES)];
    let terms = &request.search_terms[..request.search_terms.len().min(WHO_MAX_SEARCH_TERMS)];

    let mut body = Vec::with_capacity(32 + request.player_name.len() + request.guild_name.len());
    body.extend_from_slice(&request.level_min.to_le_bytes());
    body.extend_from_slice(&request.level_max.to_le_bytes());
    push_cstring(&mut body, &request.player_name);
    push_cstring(&mut body, &request.guild_name);
    body.extend_from_slice(&request.race_mask.to_le_bytes());
    body.extend_from_slice(&request.class_mask.to_le_bytes());
    body.extend_from_slice(&(zones.len() as u32).to_le_bytes());
    for zone in zones {
        body.extend_from_slice(&zone.to_le_bytes());
    }
    body.extend_from_slice(&(terms.len() as u32).to_le_bytes());
    for term in terms {
        push_cstring(&mut body, term);
    }
    body
}

/// Body of `CMSG_FRIEND_LIST`: empty. The `ShowFriends()` refresh; the list also arrives unasked
/// at login (`CharacterHandler.cpp:527`).
pub fn friend_list() -> Vec<u8> {
    Vec::new()
}

/// Body of `CMSG_ADD_FRIEND`: the name; the server normalises its case (`normalizePlayerName`).
pub fn add_friend(name: &str) -> Vec<u8> {
    cstring_body(name)
}

/// Body of `CMSG_DEL_FRIEND`: removal is by guid, not name.
pub fn del_friend(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_ADD_IGNORE`: the name to ignore.
pub fn add_ignore(name: &str) -> Vec<u8> {
    cstring_body(name)
}

/// Body of `CMSG_DEL_IGNORE`: removal is by guid.
pub fn del_ignore(guid: u64) -> Vec<u8> {
    guid.to_le_bytes().to_vec()
}

/// Body of `CMSG_SET_LOOKING_FOR_GROUP` (`0x4e88c0`): the three slot words as stored, then the
/// comment cstring, always. The 1.12 client sends it only when a commit changed something.
pub fn set_looking_for_group(slots: [u32; 3], comment: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(12 + comment.len() + 1);
    for slot in slots {
        body.extend_from_slice(&slot.to_le_bytes());
    }
    push_cstring(&mut body, comment);
    body
}

/// Body of `MSG_LOOKING_FOR_GROUP` as `LFGQuery` sends it (`0x4e97f9`..`0x4e983a`): two
/// `PutUInt32` (`0x4181f0`) after the opcode, the criterion word `(type << 24) & entry` (an `and`,
/// `0x4e9826`, so 0 for any entry below 2^24) and the class argument, 0 when it is not a number.
pub fn looking_for_group_query(criterion: u32, class: u32) -> Vec<u8> {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&criterion.to_le_bytes());
    body.extend_from_slice(&class.to_le_bytes());
    body
}

fn cstring_body(s: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(s.len() + 1);
    push_cstring(&mut body, s);
    body
}

fn push_cstring(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_looking_for_group_is_three_words_and_a_cstring() {
        let body = set_looking_for_group([0, 0, 0], "LF2M UBRS");
        assert_eq!(&body[..12], &[0u8; 12]);
        assert_eq!(&body[12..], b"LF2M UBRS\0");
        assert_eq!(set_looking_for_group([1, 2, 3], ""), {
            let mut v = vec![1u8, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0];
            v.push(0);
            v
        });
    }

    #[test]
    fn friend_list_tail_is_conditional_on_the_status_byte() {
        let mut body = vec![2u8]; // count
        body.extend_from_slice(&7u64.to_le_bytes()); // offline friend
        body.push(friend_status::OFFLINE);
        body.extend_from_slice(&9u64.to_le_bytes()); // online friend
        body.push(friend_status::AFK);
        body.extend_from_slice(&12u32.to_le_bytes()); // area
        body.extend_from_slice(&60u32.to_le_bytes()); // level
        body.extend_from_slice(&11u32.to_le_bytes()); // class

        let friends = read_friend_list(&mut &body[..]).unwrap();
        assert_eq!(
            friends,
            vec![
                FriendEntry {
                    guid: 7,
                    status: friend_status::OFFLINE,
                    ..Default::default()
                },
                FriendEntry {
                    guid: 9,
                    status: friend_status::AFK,
                    area: 12,
                    level: 60,
                    class: 11,
                },
            ]
        );
        assert!(!friends[0].is_online() && friends[1].is_online());
    }

    #[test]
    fn ignore_list_is_a_counted_guid_array() {
        let mut body = vec![2u8];
        body.extend_from_slice(&0x1122u64.to_le_bytes());
        body.extend_from_slice(&0x3344u64.to_le_bytes());
        assert_eq!(
            read_ignore_list(&mut &body[..]).unwrap(),
            vec![0x1122, 0x3344]
        );
    }

    #[test]
    fn friend_status_tail_is_keyed_on_the_result_code() {
        let mut online = vec![friend_result::ONLINE];
        online.extend_from_slice(&9u64.to_le_bytes());
        online.push(friend_status::DND);
        online.extend_from_slice(&1519u32.to_le_bytes());
        online.extend_from_slice(&60u32.to_le_bytes());
        online.extend_from_slice(&8u32.to_le_bytes());
        assert_eq!(
            read_friend_status(&mut &online[..]).unwrap(),
            FriendStatusUpdate {
                result: friend_result::ONLINE,
                guid: 9,
                online: Some(FriendOnline {
                    status: friend_status::DND,
                    area: 1519,
                    level: 60,
                    class: 8,
                }),
            }
        );

        let mut removed = vec![friend_result::REMOVED];
        removed.extend_from_slice(&9u64.to_le_bytes());
        removed.extend_from_slice(&[0xFF; 13]); // must be ignored, not parsed
        let status = read_friend_status(&mut &removed[..]).unwrap();
        assert_eq!(status.result, friend_result::REMOVED);
        assert_eq!(status.online, None);
    }

    #[test]
    fn who_results_carry_both_counts() {
        let mut body = Vec::new();
        body.extend_from_slice(&1u32.to_le_bytes()); // displayed
        body.extend_from_slice(&57u32.to_le_bytes()); // total
        body.extend_from_slice(b"Tigole\0");
        body.extend_from_slice(b"Legacy of Steel\0");
        body.extend_from_slice(&40u32.to_le_bytes()); // level
        body.extend_from_slice(&4u32.to_le_bytes()); // class
        body.extend_from_slice(&1u32.to_le_bytes()); // race
        body.extend_from_slice(&40u32.to_le_bytes()); // zone

        assert_eq!(
            read_who(&mut &body[..]).unwrap(),
            WhoResults {
                displayed: 1,
                total: 57,
                entries: vec![WhoEntry {
                    name: "Tigole".into(),
                    guild: "Legacy of Steel".into(),
                    level: 40,
                    class: 4,
                    race: 1,
                    zone: 40,
                }],
            }
        );
    }

    #[test]
    fn who_reads_the_empty_guild_string() {
        let mut body = Vec::new();
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(b"Solo\0");
        body.extend_from_slice(b"\0");
        body.extend_from_slice(&5u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&3u32.to_le_bytes());
        body.extend_from_slice(&1u32.to_le_bytes());
        let who = read_who(&mut &body[..]).unwrap();
        assert_eq!(who.entries[0].guild, "");
        assert_eq!(who.entries[0].level, 5);
    }

    #[test]
    fn who_request_body_is_byte_exact() {
        let request = WhoRequest {
            level_min: 1,
            level_max: 10,
            player_name: "bob".into(),
            guild_name: String::new(),
            race_mask: 0xFFFF_FFFF,
            class_mask: 0x0000_0002,
            zones: vec![12],
            search_terms: vec!["elw".into()],
        };
        let mut want = Vec::new();
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(&10u32.to_le_bytes());
        want.extend_from_slice(b"bob\0");
        want.extend_from_slice(b"\0");
        want.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        want.extend_from_slice(&2u32.to_le_bytes());
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(&12u32.to_le_bytes());
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(b"elw\0");
        assert_eq!(who(&request), want);
    }

    /// A bare `/who` as bytes: levels 0 to 100, both names empty, every race and class, no lists.
    #[test]
    fn the_default_who_query_encodes_levels_0_to_100_with_every_filter_open() {
        let body = who(&WhoRequest::default());
        assert_eq!(&body[0..4], &0u32.to_le_bytes(), "levelMin");
        assert_eq!(&body[4..8], &100u32.to_le_bytes(), "levelMax — not 255");
        assert_eq!(&body[8..10], b"\0\0", "both name filters empty");
        assert_eq!(&body[10..14], &u32::MAX.to_le_bytes(), "every race");
        assert_eq!(&body[14..18], &u32::MAX.to_le_bytes(), "every class");
        assert_eq!(&body[18..22], &0u32.to_le_bytes(), "no zones");
        assert_eq!(&body[22..26], &0u32.to_le_bytes(), "no terms");
        assert_eq!(body.len(), 26);
    }

    #[test]
    fn who_trims_to_the_servers_caps() {
        let request = WhoRequest {
            zones: (0..15).collect(),
            search_terms: (0..7).map(|i| i.to_string()).collect(),
            ..Default::default()
        };
        let body = who(&request);
        let zone_count = u32::from_le_bytes(body[18..22].try_into().unwrap());
        assert_eq!(zone_count as usize, WHO_MAX_ZONES);
        let after_zones = 22 + WHO_MAX_ZONES * 4;
        let term_count = u32::from_le_bytes(body[after_zones..after_zones + 4].try_into().unwrap());
        assert_eq!(term_count as usize, WHO_MAX_SEARCH_TERMS);
    }

    #[test]
    fn the_add_and_del_bodies() {
        assert_eq!(add_friend("Bob"), b"Bob\0");
        assert_eq!(add_ignore("Bob"), b"Bob\0");
        assert_eq!(del_friend(0xDEAD_BEEF), 0xDEAD_BEEFu64.to_le_bytes());
        assert_eq!(del_ignore(0xDEAD_BEEF), 0xDEAD_BEEFu64.to_le_bytes());
        assert!(friend_list().is_empty());
    }
}
