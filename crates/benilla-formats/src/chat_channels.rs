//! `ChatChannels.dbc`, the six built-in chat channels. The zone-channel join is the client's
//! (vmangos's `Player::UpdateLocalChannels`, `Objects/Player.cpp:5121`, is empty): at world entry
//! and every zone change `0x49a210` sends `CMSG_JOIN_CHANNEL` for each auto-join row.

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::chain::Chain;
use crate::dbc::{parse, str_at, u32_at};

const CHAT_CHANNELS: &str = "DBFilesClient\\ChatChannels.dbc";

/// `ChatChannels.dbc` flag bits, named as vmangos's `ChannelDBCFlags` (`Chat/Channel.h:107-116`).
pub mod flags {
    /// The client joins it on its own: General (1), Trade (2) and LocalDefense (22).
    pub const INITIAL: u32 = 0x00001;
    /// The name carries a `%s` filled per zone, so the channel is re-joined on a zone change.
    pub const ZONE_DEP: u32 = 0x00002;
    /// One channel for the whole realm (WorldDefense).
    pub const GLOBAL: u32 = 0x00004;
    /// The trade channel proper.
    pub const TRADE: u32 = 0x00008;
    /// Joined only inside a capital; Trade and GuildRecruitment carry both city bits.
    pub const CITY_ONLY: u32 = 0x00010;
    /// The second city bit; vmangos reads this one for its `CHANNEL_FLAG_CITY`.
    pub const CITY_ONLY2: u32 = 0x00020;
    /// LocalDefense and WorldDefense.
    pub const DEFENSE: u32 = 0x10000;
    /// GuildRecruitment, which needs a guild.
    pub const GUILD_REQ: u32 = 0x20000;
    /// LookingForGroup; the 1.12 row's flags are 0, so no row carries it.
    pub const LFG: u32 = 0x40000;
}

/// One `ChatChannels.dbc` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatChannelRow {
    /// `ChannelID`: a chat event's `arg7` and `JoinChannelByName`'s first return
    /// (`ChatFrame.lua:786`, `:1379`).
    pub id: u32,
    /// Bits from [`flags`].
    pub flags: u32,
    /// The enUS name pattern, `%s` intact ("General - %s").
    pub pattern: String,
    /// The enUS shortcut ("General"), the name without number or zone.
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

    /// Whether the row is joined only inside a capital; the client tests the zone's capital flag
    /// under `CITY_ONLY` alone (`0x49a3b8`/`0x49a512`).
    pub fn is_city_only(&self) -> bool {
        self.flags & flags::CITY_ONLY != 0
    }

    /// Whether this is the guild-gated row (`GUILD_REQ`), which the client finds by that bit
    /// (`0x49f140`); a manual join or leave on it turns its auto-join off (`0x49ed3d`/`0x49ef8f`).
    pub fn is_guild_recruitment(&self) -> bool {
        self.flags & flags::GUILD_REQ != 0
    }

    /// Whether the `%s` takes the city word, not the zone's name: `CITY_ONLY2`, tested apart from
    /// [`Self::is_city_only`] (`0x49a308`/`0x49a4ea`) though 1.12 rows set both bits or neither.
    pub fn takes_city_name(&self) -> bool {
        self.flags & flags::CITY_ONLY2 != 0
    }

    /// The joinable name in `zone_name`, or with `city_name` on a city-named row ("Trade - City").
    /// The city word is data: `0x4985fd` stores at `0xb4e4f0` the name of the one `AreaTable.dbc`
    /// row with `Flags & 0x200` (id 3459, "City").
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
    /// A catalog from rows given directly, for tests that run without an install.
    pub fn from_rows(rows: Vec<ChatChannelRow>) -> Self {
        ChatChannelsCatalog { rows }
    }

    pub fn rows(&self) -> &[ChatChannelRow] {
        &self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The rows the client joins by itself, in table order.
    pub fn auto_join_rows(&self) -> impl Iterator<Item = &ChatChannelRow> {
        self.rows.iter().filter(|r| r.is_auto_join())
    }

    /// The row a composed name belongs to, matched as the server does: the name contains the
    /// pattern minus its `%s` (`GetChannelEntryFor`, `Database/DBCStores.cpp:531`).
    pub fn row_for_name(&self, name: &str) -> Option<&ChatChannelRow> {
        let lower = name.to_ascii_lowercase();
        self.rows.iter().find(|r| {
            let stem = r.pattern.replacen("%s", "", 1).to_ascii_lowercase();
            !stem.is_empty() && lower.contains(&stem)
        })
    }

    /// The ChannelID behind a composed name, a chat event's `arg7`; 0 for a custom channel.
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

        // The auto-join set is the reference's `chat-cache.txt` `ZONECHANNELS` mask.
        let mask: u32 = cat.auto_join_rows().map(|r| 1 << (r.id - 1)).sum();
        assert_eq!(mask, 0x0020_0003, "General + Trade + LocalDefense");
    }

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
