//! The guild system lines: `ERR_GUILD_*` message ids the engine composes, not the FrameXML. The
//! event, invite, decline and command-result handlers (`0x5e7180`, `0x5e6f65`, `0x5e6f9a`,
//! `0x5e7520`) pass them to `CGGameUI::DisplayError 0x496720`, and the catalog row picks the
//! surface and the sound. The `GUILD_MOTD` line is the FrameXML's (`ChatFrame.lua:1335-1340`) and
//! `/ginfo`'s are not catalog rows (built in [`super::feed`]), so neither is here.

use benilla_protocol::messages::{
    guild_command, guild_command_error, guild_event, GuildCommandResult, GuildEventNotice,
};

use crate::ui_action::UiError;

/// The strings the event handler's shared tail passes to `0x496720`: one to three as they arrived,
/// none for a count of 0 or 4 and up (`0x5e745f`). Never padded: `SStrPrintf` copies a starved
/// `%s` through as written, so a two-string promotion ends "to %s.".
fn emitted_params(notice: &GuildEventNotice) -> Vec<&str> {
    match notice.params.len() {
        1..=3 => notice.params.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

/// The line one `SMSG_GUILD_EVENT` prints, if any; ids map to keys as the handler's case arms do
/// (`0x5e720a` to `0x5e745a`). `announce_signon` is the sign-on/sign-off pair's whole display
/// condition, decided in `super::net::event`; no other arm has one.
pub(super) fn event_line(notice: &GuildEventNotice, announce_signon: bool) -> Option<UiError> {
    let args = emitted_params(notice);
    let shared = |key: &'static str| Some(UiError::strings(key, &args));
    match notice.event {
        guild_event::PROMOTION => shared("ERR_GUILD_PROMOTE_SSS"),
        guild_event::DEMOTION => shared("ERR_GUILD_DEMOTE_SSS"),
        guild_event::JOINED => shared("ERR_GUILD_JOIN_S"),
        guild_event::LEFT => shared("ERR_GUILD_LEAVE_S"),
        guild_event::REMOVED => shared("ERR_GUILD_REMOVE_SS"),
        guild_event::LEADER_IS => shared("ERR_GUILD_LEADER_IS_S"),
        guild_event::LEADER_CHANGED => shared("ERR_GUILD_LEADER_CHANGED_SS"),
        guild_event::DISBANDED => shared("ERR_GUILD_DISBANDED"),
        // Outside the shared tail: `0x106` takes the one name twice, for the `|Hplayer:%s|h` link
        // and the bracketed display name.
        guild_event::SIGNED_ON if announce_signon => {
            let name = notice
                .params
                .first()
                .map(String::as_str)
                .unwrap_or_default();
            Some(UiError::strings("ERR_FRIEND_ONLINE_SS", &[name, name]))
        }
        guild_event::SIGNED_OFF if announce_signon => {
            let name = notice
                .params
                .first()
                .map(String::as_str)
                .unwrap_or_default();
            Some(UiError::strings("ERR_FRIEND_OFFLINE_S", &[name]))
        }
        // Silent: the MOTD line is the FrameXML's, a rank rename and a roster update print
        // nothing, and neither does a refused sign-on.
        guild_event::MOTD | guild_event::UPDATE_RANK_NAME | guild_event::UPDATE_ROSTER => None,
        guild_event::SIGNED_ON | guild_event::SIGNED_OFF => None,
        // `0x09` and every id past `0x0d` share the default arm, which prints `0x69`,
        // `ERR_GUILD_INTERNAL`, through the shared tail (`0x5e745a`).
        _ => shared("ERR_GUILD_INTERNAL"),
    }
}

/// The line one `SMSG_GUILD_COMMAND_RESULT` prints, if any: on result 0 the [`guild_command`]
/// tag picks it (`0x5e7550`), otherwise the result code does. Every `_S` key takes the name and
/// every other key nothing (`0x5e7520`).
pub(super) fn command_line(result: &GuildCommandResult) -> Option<UiError> {
    let name = result.name.as_str();
    let named = |key: &'static str| Some(UiError::s(key, name));
    if result.result == guild_command_error::PLAYER_NO_MORE_IN_GUILD {
        return match result.command {
            guild_command::CREATE => named("ERR_GUILD_CREATE_S"),
            guild_command::INVITE => named("ERR_GUILD_INVITE_S"),
            guild_command::QUIT => named("ERR_GUILD_QUIT_S"),
            guild_command::FOUNDER => named("ERR_GUILD_FOUNDER_S"),
            // 19 and 20 only re-request the roster; the rest have no effect
            // (vmangos `Guild/Guild.h:84`).
            _ => None,
        };
    }
    match result.result {
        guild_command_error::INTERNAL => Some(UiError::key("ERR_GUILD_INTERNAL")),
        guild_command_error::ALREADY_IN_GUILD => Some(UiError::key("ERR_ALREADY_IN_GUILD")),
        guild_command_error::ALREADY_IN_GUILD_S => named("ERR_ALREADY_IN_GUILD_S"),
        guild_command_error::INVITED_TO_GUILD => Some(UiError::key("ERR_INVITED_TO_GUILD")),
        guild_command_error::ALREADY_INVITED_TO_GUILD_S => named("ERR_ALREADY_INVITED_TO_GUILD_S"),
        guild_command_error::NAME_INVALID => Some(UiError::key("ERR_GUILD_NAME_INVALID")),
        guild_command_error::NAME_EXISTS_S => named("ERR_GUILD_NAME_EXISTS_S"),
        // `0x08` is `ERR_GUILD_LEADER_LEAVE` under `QUIT` and `ERR_GUILD_PERMISSIONS` otherwise.
        guild_command_error::PERMISSIONS if result.command == guild_command::QUIT => {
            Some(UiError::key("ERR_GUILD_LEADER_LEAVE"))
        }
        guild_command_error::PERMISSIONS => Some(UiError::key("ERR_GUILD_PERMISSIONS")),
        guild_command_error::PLAYER_NOT_IN_GUILD => {
            Some(UiError::key("ERR_GUILD_PLAYER_NOT_IN_GUILD"))
        }
        guild_command_error::PLAYER_NOT_IN_GUILD_S => named("ERR_GUILD_PLAYER_NOT_IN_GUILD_S"),
        guild_command_error::PLAYER_NOT_FOUND_S => named("ERR_GUILD_PLAYER_NOT_FOUND_S"),
        guild_command_error::NOT_ALLIED => Some(UiError::key("ERR_GUILD_NOT_ALLIED")),
        guild_command_error::RANK_TOO_HIGH_S => named("ERR_GUILD_RANK_TOO_HIGH_S"),
        guild_command_error::RANK_TOO_LOW_S => named("ERR_GUILD_RANK_TOO_LOW_S"),
        guild_command_error::RANKS_LOCKED => Some(UiError::key("ERR_GUILD_RANKS_LOCKED")),
        guild_command_error::RANK_IN_USE => Some(UiError::key("ERR_GUILD_RANK_IN_USE")),
        guild_command_error::IGNORING_YOU_S => named("ERR_IGNORING_YOU_S"),
        // Silent: 15, 16, `UNK20` (a roster re-request for command 5) and anything unknown.
        _ => None,
    }
}

/// `SMSG_GUILD_INVITE`'s notice line, printed beside the popup (`0x5e6f65`, message `0x4f`); the
/// popup is the separate `GUILD_INVITE_REQUEST` at `0x5e6f53`.
pub(super) fn invite_line(inviter: &str, guild: &str) -> UiError {
    UiError::strings("ERR_INVITED_TO_GUILD_SS", &[inviter, guild])
}

/// `SMSG_GUILD_DECLINE`: our invitee said no (sent to the inviter only).
pub(super) fn decline_line(name: &str) -> UiError {
    UiError::s("ERR_GUILD_DECLINE_S", name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::messages::{by_key, MsgKind};

    fn notice(event: u8, params: &[&str]) -> GuildEventNotice {
        GuildEventNotice {
            event,
            params: params.iter().map(|s| (*s).to_string()).collect(),
            guid: None,
        }
    }

    fn strings(e: &UiError) -> Vec<&str> {
        e.args
            .iter()
            .map(|a| match a {
                crate::ui_action::FillArg::S(s) => s.as_str(),
                crate::ui_action::FillArg::D(_) => unreachable!("guild lines push no integers"),
            })
            .collect()
    }

    #[test]
    fn the_multi_slot_lines_name_the_key_and_pass_their_strings_in_order() {
        let e = event_line(
            &notice(guild_event::PROMOTION, &["Tigole", "Furor", "Officer"]),
            false,
        )
        .expect("a promotion prints");
        assert_eq!(e.key, "ERR_GUILD_PROMOTE_SSS");
        assert_eq!(strings(&e), ["Tigole", "Furor", "Officer"]);
    }

    #[test]
    fn the_emitter_tail_passes_one_two_or_three_strings_and_otherwise_none() {
        for (params, want) in [
            (&["a"][..], 1usize),
            (&["a", "b"][..], 2),
            (&["a", "b", "c"][..], 3),
            (&[][..], 0),
            (&["a", "b", "c", "d"][..], 0),
        ] {
            let e = event_line(&notice(guild_event::PROMOTION, params), false).expect("prints");
            assert_eq!(e.args.len(), want, "strCount {}", params.len());
        }
    }

    #[test]
    fn sign_on_lines_are_the_friend_lines_and_obey_their_condition() {
        let on = event_line(&notice(guild_event::SIGNED_ON, &["Tigole"]), true).expect("prints");
        assert_eq!(on.key, "ERR_FRIEND_ONLINE_SS");
        assert_eq!(
            strings(&on),
            ["Tigole", "Tigole"],
            "the link and the bracket"
        );
        let off = event_line(&notice(guild_event::SIGNED_OFF, &["Tigole"]), true).expect("prints");
        assert_eq!(off.key, "ERR_FRIEND_OFFLINE_S");
        assert_eq!(strings(&off), ["Tigole"]);

        for event in [guild_event::SIGNED_ON, guild_event::SIGNED_OFF] {
            assert!(
                event_line(&notice(event, &["Tigole"]), false).is_none(),
                "a refused condition prints nothing at all"
            );
        }
    }

    #[test]
    fn only_the_three_roster_events_are_silent() {
        for event in [
            guild_event::MOTD,
            guild_event::UPDATE_RANK_NAME,
            guild_event::UPDATE_ROSTER,
        ] {
            assert_eq!(
                event_line(&notice(event, &["x"]), false),
                None,
                "{event:#04x}"
            );
        }
        // `0x09` and every id past `0x0d` share the default arm, which pushes `0x69`.
        for event in [guild_event::TABARD_CHANGE, 0x0e, 0x77, 0xff] {
            let e = event_line(&notice(event, &["x"]), false).expect("the default arm prints");
            assert_eq!(e.key, "ERR_GUILD_INTERNAL", "{event:#04x}");
        }
    }

    #[test]
    fn the_two_meanings_of_result_eight_are_told_apart_by_the_command() {
        let quit = command_line(&GuildCommandResult {
            command: guild_command::QUIT,
            name: String::new(),
            result: guild_command_error::LEADER_LEAVE,
        })
        .expect("prints");
        assert_eq!(quit.key, "ERR_GUILD_LEADER_LEAVE");
        let other = command_line(&GuildCommandResult {
            command: guild_command::INVITE,
            name: String::new(),
            result: guild_command_error::PERMISSIONS,
        })
        .expect("prints");
        assert_eq!(other.key, "ERR_GUILD_PERMISSIONS");
    }

    #[test]
    fn result_zero_is_the_success_side() {
        let invited = command_line(&GuildCommandResult {
            command: guild_command::INVITE,
            name: "Kaplan".into(),
            result: guild_command_error::PLAYER_NO_MORE_IN_GUILD,
        })
        .expect("prints");
        assert_eq!(invited.key, "ERR_GUILD_INVITE_S");
        assert_eq!(strings(&invited), ["Kaplan"]);
        assert!(
            command_line(&GuildCommandResult {
                command: 0x99,
                name: String::new(),
                result: guild_command_error::PLAYER_NO_MORE_IN_GUILD,
            })
            .is_none(),
            "a command with no message says nothing"
        );
    }

    #[test]
    fn every_underscore_s_key_takes_the_name_and_every_other_takes_nothing() {
        for result in 0u32..=0x15 {
            for command in [
                guild_command::CREATE,
                guild_command::INVITE,
                guild_command::QUIT,
            ] {
                let Some(e) = command_line(&GuildCommandResult {
                    command,
                    name: "Kaplan".into(),
                    result,
                }) else {
                    continue;
                };
                let want = usize::from(e.key.ends_with("_S"));
                assert_eq!(e.args.len(), want, "{} (result {result}) ", e.key);
            }
        }
    }

    #[test]
    fn every_key_is_a_catalog_row_and_two_of_them_are_the_red_line() {
        let mut seen = Vec::new();
        for event in 0u8..=0x0f {
            for announce in [true, false] {
                seen.extend(event_line(&notice(event, &["a", "b", "c"]), announce));
            }
        }
        for result in 0u32..=0x15 {
            for command in 0u32..=0x14 {
                seen.extend(command_line(&GuildCommandResult {
                    command,
                    name: "Kaplan".into(),
                    result,
                }));
            }
        }
        seen.push(invite_line("Tigole", "Legacy of Steel"));
        seen.push(decline_line("Kaplan"));
        assert!(seen.len() > 20, "the sweep found the table");
        for e in &seen {
            assert!(by_key(e.key).is_some(), "{} is not a catalog row", e.key);
        }

        // The two name refusals are `kind 2`, the red `UI_ERROR_MESSAGE`, not chat.
        for key in ["ERR_GUILD_NAME_INVALID", "ERR_GUILD_NAME_EXISTS_S"] {
            assert_eq!(by_key(key).map(|r| r.kind), Some(MsgKind::Error), "{key}");
        }
        // The rest of the guild family is chat.
        assert_eq!(
            by_key("ERR_GUILD_PROMOTE_SSS").map(|r| r.kind),
            Some(MsgKind::Chat)
        );
    }

    #[test]
    fn the_invite_notice_names_both() {
        let e = invite_line("Tigole", "Legacy of Steel");
        assert_eq!(e.key, "ERR_INVITED_TO_GUILD_SS");
        assert_eq!(strings(&e), ["Tigole", "Legacy of Steel"]);
        assert_eq!(decline_line("Kaplan").key, "ERR_GUILD_DECLINE_S");
    }
}
