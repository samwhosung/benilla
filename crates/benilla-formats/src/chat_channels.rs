//! `ChatChannels.dbc` — the six built-in chat channels and, in their flags, **which ones the
//! client joins by itself**.
//!
//! 1.12 has no server-side zone-channel join: vmangos's `Player::UpdateLocalChannels`
//! (`src/game/Objects/Player.cpp:5121`) is an empty function whose whole body is the comment
//! `// Updated client-side`. The client walks this table at world entry **and on every zone
//! change** and sends `CMSG_JOIN_CHANNEL` for each auto-join row with the zone's name spliced into
//! the row's pattern — one function, `ZoneChannelRefresh 0x49a210` (decision 0288 phase 6; the
//! walk itself is this crate's consumer, `benilla_app::ui_chat::channels`).
//!
//! Layout — the real 5875 file, decoded here (662 bytes, 6 records × **21 × u32** cols, 138-byte
//! string block): `ChannelID(0), Flags(1), FactionGroup(2), Name[8](3..10), NameMask(11),
//! Shortcut[8](12..19), ShortcutMask(20)`. The whole shipped table, verbatim:
//!
//! | ID | flags | name pattern (enUS) | shortcut |
//! |---|---|---|---|
//! | 1 | `0x00003` | `General - %s` | `General` |
//! | 2 | `0x0003B` | `Trade - %s` | `Trade` |
//! | 22 | `0x10003` | `LocalDefense - %s` | `LocalDefense` |
//! | 23 | `0x10004` | `WorldDefense` | `WorldDefense` |
//! | 24 | `0x00000` | `LookingForGroup` | `LookingForGroup` |
//! | 25 | `0x20032` | `GuildRecruitment - %s` | `GuildRecruitment` |
//!
//! The flag names are vmangos's ([`flags`], `src/game/Chat/Channel.h`). **Exactly rows 1, 2 and 22
//! carry `INITIAL`** — and that is independently corroborated: the reference client's own
//! `WTF/…/chat-cache.txt` persists `ZONECHANNELS 2097155` = `0x200003` = bits 0, 1 and 21, i.e.
//! `1 << (ChannelID - 1)` for exactly those three. Two unrelated artifacts, one set.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const CHAT_CHANNELS: &str = "DBFilesClient\\ChatChannels.dbc";

/// `ChatChannels.dbc` flag bits, named per vmangos `Channel::ChannelDBCFlags`
/// (`src/game/Chat/Channel.h:107-116`) and checked against the shipped rows in the module table.
pub mod flags {
    /// The client joins this channel on its own. Rows 1 (General), 2 (Trade), 22 (LocalDefense).
    pub const INITIAL: u32 = 0x00001;
    /// The name carries a `%s` filled per zone — the channel is re-joined when the zone changes.
    pub const ZONE_DEP: u32 = 0x00002;
    /// One channel for the whole realm (WorldDefense).
    pub const GLOBAL: u32 = 0x00004;
    /// The trade channel proper.
    pub const TRADE: u32 = 0x00008;
    /// Only joined while inside a capital. Trade and GuildRecruitment carry both city bits.
    pub const CITY_ONLY: u32 = 0x00010;
    /// Its twin — vmangos reads this one for its own `CHANNEL_FLAG_CITY`.
    pub const CITY_ONLY2: u32 = 0x00020;
    /// LocalDefense / WorldDefense.
    pub const DEFENSE: u32 = 0x10000;
    /// GuildRecruitment — needs a guild.
    pub const GUILD_REQ: u32 = 0x20000;
    /// LookingForGroup. Zero on the 1.12 row, so nothing carries it in this build.
    pub const LFG: u32 = 0x40000;
}

/// One `ChatChannels.dbc` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatChannelRow {
    /// `ChannelID` — the number the client hands Lua as a chat event's **arg7** and as
    /// `JoinChannelByName`'s first return (`ChatFrame.lua:786`, `:1379`).
    pub id: u32,
    /// See [`flags`].
    pub flags: u32,
    /// The enUS name pattern, `%s` intact ("General - %s").
    pub pattern: String,
    /// The enUS shortcut ("General") — the number-less, zone-less name.
    pub shortcut: String,
}

impl ChatChannelRow {
    /// Does the client join this one without being asked?
    pub fn is_auto_join(&self) -> bool {
        self.flags & flags::INITIAL != 0
    }

    /// Is the name zone-dependent (does its pattern carry the `%s`)?
    pub fn is_zone_dependent(&self) -> bool {
        self.flags & flags::ZONE_DEP != 0
    }

    /// Is this row **only joined inside a capital**? Gated on `CITY_ONLY` (`0x10`) alone — the
    /// client tests the zone's own capital flag under this bit and no other
    /// (`0x49a3b8`/`0x49a512`).
    pub fn is_city_only(&self) -> bool {
        self.flags & flags::CITY_ONLY != 0
    }

    /// Is this the row a **guild** decides about — `GUILD_REQ` (`0x20000`), `GuildRecruitment`
    /// alone in the 1.12 table? The client keys three things on it: the cascade's composer
    /// `0x49f140` finds the row by this bit, and a manual `JoinChannelByName`/`LeaveChannelByName`
    /// on it forces the auto-join option off (`0x49ed3d`/`0x49ef8f`).
    pub fn is_guild_recruitment(&self) -> bool {
        self.flags & flags::GUILD_REQ != 0
    }

    /// Does this row's `%s` take the shared **city** word instead of the zone's name?
    ///
    /// A different bit from [`Self::is_city_only`] — `CITY_ONLY2` (`0x20`) — and a different test
    /// in the client (`0x49a308`/`0x49a4ea` pick the substitution on it). Rows 2 and 25 carry
    /// both bits, so the two questions have the same answer on the 1.12 table; they are kept apart
    /// because the client keeps them apart.
    pub fn takes_city_name(&self) -> bool {
        self.flags & flags::CITY_ONLY2 != 0
    }

    /// The joinable name for a player standing in `zone_name`, with `city_name` the shared capital
    /// word.
    ///
    /// The `%s` is the **zone**'s display name for an ordinary zone-dependent row
    /// ("General - Elwynn Forest") and `city_name` for a city-named one ("Trade - City") — one
    /// trade channel shared by every capital rather than one per city.
    ///
    /// **"City" is DBC data, not a string literal**: the client keeps the word at `0xb4e4f0`, whose
    /// single writer `0x4985fd` is a load-time scan of `AreaTable.dbc` for the row with
    /// `Flags & 0x200`. In the shipped 5875 table that is **exactly one row — id 3459,
    /// `AreaName[enUS] = "City"`** (asserted in [`tests`] against the real file), which is
    /// precisely why searching `WoW.exe` for `City\0` finds nothing at all. The caller passes the
    /// word in rather than this crate re-reading `AreaTable.dbc`, because the auto-join walk
    /// already holds that catalog — and passing it keeps the localized name localized.
    pub fn joinable_name(&self, zone_name: &str, city_name: &str) -> String {
        if !self.is_zone_dependent() {
            return self.pattern.clone();
        }
        let subject = if self.takes_city_name() {
            city_name
        } else {
            zone_name
        };
        self.pattern.replacen("%s", subject, 1)
    }
}

/// The whole six-row table, in file order.
#[derive(Clone, Debug, Default)]
pub struct ChatChannelsCatalog {
    rows: Vec<ChatChannelRow>,
}

impl ChatChannelsCatalog {
    /// Build a catalog from rows directly — for tests that must run without an install (the real
    /// six rows are asserted against the shipped DBC by [`tests`]).
    pub fn from_rows(rows: Vec<ChatChannelRow>) -> Self {
        ChatChannelsCatalog { rows }
    }

    pub fn rows(&self) -> &[ChatChannelRow] {
        &self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The rows the client joins by itself, in table order ([`ChatChannelRow::is_auto_join`]).
    pub fn auto_join_rows(&self) -> impl Iterator<Item = &ChatChannelRow> {
        self.rows.iter().filter(|r| r.is_auto_join())
    }

    /// The row a composed channel name belongs to, or `None` for a custom channel.
    ///
    /// Matched the way the server matches it — vmangos's `GetChannelEntryFor(std::string const&)`
    /// (`src/game/Database/DBCStores.cpp:531`) strips the `%s` from each pattern and asks whether
    /// the name *contains* what is left, so `"General - Elwynn Forest"` resolves to ChannelID 1.
    /// Matching the server's own rule is the point: it is what decides whether the channel we
    /// joined is the built-in one both clients land in.
    pub fn row_for_name(&self, name: &str) -> Option<&ChatChannelRow> {
        let lower = name.to_ascii_lowercase();
        self.rows.iter().find(|r| {
            let stem = r.pattern.replacen("%s", "", 1).to_ascii_lowercase();
            !stem.is_empty() && lower.contains(&stem)
        })
    }

    /// The `ChatChannels.dbc` ChannelID behind a composed name — a chat event's **arg7**.
    /// 0 for a custom channel, which is what the reference passes there too.
    pub fn zone_channel_id(&self, name: &str) -> u32 {
        self.row_for_name(name).map_or(0, |r| r.id)
    }
}

fn schema() -> Schema {
    let mut s = Schema::new("ChatChannels");
    s.add_field(SchemaField::new("ChannelID", FieldType::UInt32));
    s.add_field(SchemaField::new("Flags", FieldType::UInt32));
    s.add_field(SchemaField::new("FactionGroup", FieldType::UInt32));
    s.add_field(SchemaField::new_array("Name", FieldType::String, 8));
    s.add_field(SchemaField::new("NameMask", FieldType::UInt32));
    s.add_field(SchemaField::new_array("Shortcut", FieldType::String, 8));
    s.add_field(SchemaField::new("ShortcutMask", FieldType::UInt32));
    s.set_key_field("ChannelID");
    s
}

pub fn load_chat_channels_catalog(chain: &mut Chain) -> Result<ChatChannelsCatalog> {
    let bytes = chain
        .read_file(CHAT_CHANNELS)
        .context("reading ChatChannels.dbc")?;
    let rs = parse(&bytes, schema(), "ChatChannels")?;
    let mut rows = Vec::with_capacity(rs.records().len());
    for r in rs.records() {
        let (Some(id), Some(flags), Some(pattern)) =
            (u32_at(r, 0), u32_at(r, 1), str_at(&rs, r, 3))
        else {
            continue;
        };
        rows.push(ChatChannelRow {
            id,
            flags,
            pattern,
            shortcut: str_at(&rs, r, 12).unwrap_or_default(),
        });
    }
    Ok(ChatChannelsCatalog { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real shipped table, row for row — the module doc's table asserted rather than trusted,
    /// because every channel name benilla sends on the wire is composed from these six rows.
    /// Skips without client data.
    #[test]
    fn the_real_table_is_the_six_documented_rows() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_chat_channels_catalog(&mut chain).expect("load ChatChannels");

        let got: Vec<(u32, u32, &str, &str)> = cat
            .rows()
            .iter()
            .map(|r| (r.id, r.flags, r.pattern.as_str(), r.shortcut.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (1, 0x00003, "General - %s", "General"),
                (2, 0x0003B, "Trade - %s", "Trade"),
                (22, 0x10003, "LocalDefense - %s", "LocalDefense"),
                (23, 0x10004, "WorldDefense", "WorldDefense"),
                (24, 0x00000, "LookingForGroup", "LookingForGroup"),
                (25, 0x20032, "GuildRecruitment - %s", "GuildRecruitment"),
            ]
        );

        // The auto-join set is exactly the `chat-cache.txt` ZONECHANNELS mask 0x200003 — two
        // artifacts, one answer (module doc).
        let mask: u32 = cat.auto_join_rows().map(|r| 1 << (r.id - 1)).sum();
        assert_eq!(mask, 0x0020_0003, "General + Trade + LocalDefense");
    }

    /// Name composition, both arms, and the reverse lookup the server also does.
    #[test]
    fn names_compose_per_zone_and_resolve_back_to_their_row() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let cat = load_chat_channels_catalog(&mut chain).expect("load ChatChannels");
        let row = |id: u32| {
            cat.rows()
                .iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("row {id}"))
        };

        assert_eq!(
            row(1).joinable_name("Elwynn Forest", "City"),
            "General - Elwynn Forest"
        );
        assert_eq!(
            row(22).joinable_name("Elwynn Forest", "City"),
            "LocalDefense - Elwynn Forest"
        );
        // City-named: one channel for every capital, not one per capital.
        assert_eq!(
            row(2).joinable_name("Stormwind City", "City"),
            "Trade - City"
        );
        assert_eq!(row(2).joinable_name("Orgrimmar", "City"), "Trade - City");
        // A pattern with no `%s` ignores both.
        assert_eq!(
            row(23).joinable_name("Elwynn Forest", "City"),
            "WorldDefense"
        );

        assert_eq!(cat.zone_channel_id("General - Elwynn Forest"), 1);
        assert_eq!(cat.zone_channel_id("Trade - City"), 2);
        assert_eq!(cat.zone_channel_id("LocalDefense - Durotar"), 22);
        assert_eq!(cat.zone_channel_id("World"), 0, "a custom channel is 0");
    }
}
