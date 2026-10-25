//! The guild roster's rows: a member resolved for display. Names resolve engine-side as in the
//! reference: `GetGuildRosterInfo 0x4d1200` returns the localized class and zone names, not ids
//! (`0x4d1355`, `0x4d1391`), and the rank name from the cached guild record. The getters read the
//! rows through the call host ([`super::live`]) and the verbs act in their calls
//! ([`super::GuildSteps`]).

use benilla_formats::AreaTableCatalog;
use benilla_protocol::messages::{GuildRosterMember, GUILD_RANK_MAX_LENGTH};
use benilla_ui::script::GuildMemberInfo;
use bevy::prelude::*;

use crate::ui_unit::UnitNames;

use super::{last_online, Identity, RosterRow};

/// The guilds `GetGuildInfo` missed that a harness's VM kept, each asked with its waiter as the
/// host asks it inside the call (`0x4c93d2`).
#[cfg(test)]
pub(super) fn ask_kept_guilds(
    mut script: NonSendMut<benilla_ui::script::UiScript>,
    guild: Res<super::GuildState>,
    commands: Res<crate::net::NetCommands>,
) {
    for (guild_id, guid) in script.take_guild_info_asks() {
        guild.wait_for_info(guild_id, guid, &commands);
    }
}

/// One roster row, ids resolved. An offline member keeps level, class and zone, which the wire
/// carries for every member, so the pane greys the row rather than blanking it.
pub(super) fn row(
    m: &GuildRosterMember,
    areas: Option<&AreaTableCatalog>,
    identity: Option<&Identity>,
    names: UnitNames<'_>,
) -> RosterRow {
    let online = m.is_online();
    RosterRow {
        guid: m.guid,
        last_online_days: m.last_online_days,
        info: GuildMemberInfo {
            guid: m.guid,
            name: m.name.clone(),
            rank: identity
                .map(|i| i.rank_name(m.rank_id).to_string())
                .unwrap_or_default(),
            rank_index: m.rank_id,
            level: u32::from(m.level),
            class: names
                .class(m.class)
                .map(|(display, _)| display.to_string())
                .unwrap_or_default(),
            zone: areas
                .and_then(|a| a.name(m.zone))
                .unwrap_or_default()
                .to_string(),
            note: m.public_note.clone(),
            officer_note: m.officer_note.clone(),
            online,
            presence: m.presence,
            // No float on the wire for an online member; the formatter reads all zeroes as nil.
            last_online: if online {
                Default::default()
            } else {
                last_online(m.last_online_days)
            },
        },
    }
}

/// The rank name, or `None` past vmangos's cap of 15, where it kicks the session
/// (`Handlers/GuildHandler.cpp:580-584`, `:600-604`); the stock edit box stops at 15. Deviation: a
/// script's 16-letter name, which the reference's bindings pass (1 to 16 units, `0x4d20d0`,
/// `0x4d2210`), is dropped rather than sent or truncated, because vmangos would kick the session.
pub(super) fn capped_rank_name(name: String) -> Option<String> {
    if name.chars().count() > GUILD_RANK_MAX_LENGTH {
        warn!(
            "guild: refusing a {}-character rank name (the server kicks over {GUILD_RANK_MAX_LENGTH})",
            name.chars().count()
        );
        return None;
    }
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_protocol::messages::guild_presence;

    #[test]
    fn online_is_the_whole_byte() {
        let member = |presence| GuildRosterMember {
            presence,
            ..Default::default()
        };
        assert!(member(guild_presence::ONLINE).is_online());
        assert!(member(guild_presence::AFK).is_online(), "no ONLINE bit set");
        assert!(member(0x80).is_online(), "an unknown bit reads as online");
        assert!(!member(guild_presence::OFFLINE).is_online());
    }

    #[test]
    fn an_offline_row_keeps_its_level_class_and_zone() {
        let member = GuildRosterMember {
            guid: 5,
            presence: guild_presence::OFFLINE,
            name: "Kaplan".into(),
            rank_id: 1,
            level: 42,
            class: 8, // Mage
            zone: 12,
            last_online_days: 3.5,
            public_note: "alt".into(),
            officer_note: String::new(),
        };
        let korean = crate::ui_unit::TestNames::korean();
        let row = row(&member, None, None, korean.names());
        assert_eq!(row.info.level, 42);
        assert_eq!(
            row.info.class, "마법사",
            "the table's word at the install's locale"
        );
        assert!(!row.info.online);
        assert_eq!(row.info.last_online.days, 3);
        assert_eq!(row.info.presence, guild_presence::OFFLINE);
        assert_eq!(row.guid, 5);
    }

    #[test]
    fn an_online_row_reports_no_last_online() {
        let member = GuildRosterMember {
            presence: guild_presence::ONLINE,
            // Ignored for an online member, as in the reference (`0x4d14fd`).
            last_online_days: 9.0,
            ..Default::default()
        };
        assert_eq!(
            row(&member, None, None, Default::default())
                .info
                .last_online,
            Default::default()
        );
    }

    #[test]
    fn an_over_long_rank_name_is_refused() {
        assert_eq!(
            capped_rank_name("Officer".into()).as_deref(),
            Some("Officer")
        );
        assert!(capped_rank_name("A".repeat(GUILD_RANK_MAX_LENGTH)).is_some());
        assert_eq!(
            capped_rank_name("A".repeat(GUILD_RANK_MAX_LENGTH + 1)),
            None
        );
    }
}
