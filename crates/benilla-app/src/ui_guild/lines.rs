//! The guild session's **system lines** — the `ERR_GUILD_*` messages the engine composes.
//!
//! Every guild event and every command verdict prints one line, and the *engine* composes it, not
//! the FrameXML: `SMSG_GUILD_EVENT`'s handler `0x5e7180` routes its arms into
//! `CGGameUI::DisplayError 0x496720`, and the invite / decline / command-result handlers do the
//! same (`0x5e6f65`, `0x5e6f9a`, `0x5e7520`). Not one of the `ERR_GUILD_*` keys those resolve to
//! appears anywhere in the reference FrameXML, which is what identifies them as engine-composed —
//! [`crate::ui_social`]'s own test, applied again.
//!
//! **These are message ids, not sentences** (decision 2054). Each arm names a catalog key and the
//! arguments the reference pushes beside it; the drain resolves the key against the player's own
//! `GlobalStrings.lua` and the catalog row decides the surface and the sound. That is not a
//! restatement of what was here before: this module used to carry 34 re-typed English constants and
//! its own two `fill` helpers, which threw away three things the catalog knows —
//! `ERR_GUILD_NAME_INVALID` and `ERR_GUILD_NAME_EXISTS_S` are `kind 2`, the **red** error line and
//! not chat at all; `ERR_GUILD_CREATE_S` and `ERR_INVITED_TO_GUILD_SS` carry the `LEVELUP` cue; and
//! `ERR_GUILD_PERMISSIONS` has `type_tag 0x3e`, under `VOCAL_UI_LINES`, so the reference *speaks* it
//! (decision 1815).
//!
//! **The `GUILD_MOTD` line is deliberately not here.** The reference's `ChatFrame_OnEvent` composes
//! `GUILD_MOTD_TEMPLATE` itself off the `GUILD_MOTD` event (`ChatFrame.lua:1335-1340`), so firing
//! the event is the whole job and composing a line beside it would double it.
//!
//! **`/ginfo` is not here either, and that is faithful.** Its two lines are `GUILD_NAME_TEMPLATE`
//! and `GUILD_INFO_TEMPLATE` — *not* catalog rows: `0x5e6fb0` resolves each token through the
//! script VM (`0x703bf0` at `0x5e700f`/`0x5e706b`), formats it, and emits chat directly, never
//! passing a message record. A route with no record has no `kind` and no sound to read, so it is
//! resolved where the VM is — [`super::feed`] — rather than queued as a message.
//!
//! The id → key mapping is **the handler's own case arms** (`0x5e720a`→0x59 … `0x5e745a`→0x69),
//! and the keys are matched against benilla's generated catalog rather than by name.

use benilla_protocol::messages::{
    guild_command, guild_command_error, guild_event, GuildCommandResult, GuildEventNotice,
};

use crate::ui_action::UiError;

/// The strings `SMSG_GUILD_EVENT`'s shared emitter tail passes to `0x496720`.
///
/// **`strCount == 0` and `strCount >= 4` both pass ZERO strings** (`0x5e745f`) — the tail has
/// three arms for 1, 2 and 3 and no other. That is not the
/// same as padding the missing slots with empties, which is what this module did before: a template
/// whose specifier runs out of arguments has the specifier **copied through verbatim**
/// (`SStrPrintf`, and [`benilla_ui::strings::fill`] implements it), so a two-string promotion reads
/// "Tigole has promoted Furor to %s." and not "…to .".
///
/// (The reference's read loop is *not* capped at three — a `strCount >= 4` writes past its three
/// `0x100` stack buffers into the live locals. benilla's parser reads into a `Vec`, so the overflow
/// has nothing to reproduce; the display rule is what carries over.)
fn emitted_params(notice: &GuildEventNotice) -> Vec<&str> {
    match notice.params.len() {
        1..=3 => notice.params.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

/// The message one `SMSG_GUILD_EVENT` prints, if any.
///
/// `announce_signon` is the sign-on/sign-off pair's **whole display condition**, resolved by the
/// caller because three of its four conjuncts need state this function does not see. See
/// [`super::apply::event`] for the conjuncts and their byte addresses; what matters here is that
/// the pair is the only place in this table with a condition at all, and that the guid those two
/// arms carry exists to answer it.
///
/// **This argument used to be `ignored`, and that was a mislabel** (corrected 1589):
/// `0x5ae810` is `FriendList::FindFriendSlot`, a **friends-list**
/// membership test — base `this+8`, stride `0x20`, bound `0x32` — not the ignore-list check at
/// `this+0x650`. Reading it as "ignore" got the behaviour backwards on both sides: an *ignored*
/// guildmate was silenced where the reference announces them, and a guildmate who is also a
/// *friend* was announced twice, because `SMSG_FRIEND_STATUS` says the same thing with no gate of
/// its own. `0x5ae810` has exactly two callers image-wide, these two arms.
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
        // The sign-on/sign-off pair leaves the shared tail: each has its own emit with its own
        // argument shape, and `0x106` takes the ONE name TWICE — once for the `|Hplayer:%s|h`
        // link, once for the bracketed display name.
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
        // MOTD's line is the FrameXML's (module doc); `UPDATE_RANK_NAME` warms a rank name and
        // `UPDATE_ROSTER` only re-requests, and neither prints; a sign-on the condition refuses
        // shows nothing.
        guild_event::MOTD | guild_event::UPDATE_RANK_NAME | guild_event::UPDATE_ROSTER => None,
        guild_event::SIGNED_ON | guild_event::SIGNED_OFF => None,
        // **`0x09` and everything past `0x0d` fall to the SAME arm**, and it is not silence:
        // `0x5e745a` pushes `0x69` = `ERR_GUILD_INTERNAL` and jumps into the shared tail like the
        // rest. This module used to return `None` here under a comment
        // saying the key "is not settled" — it is settled, in the reference and in benilla's own
        // generated catalog, and `TABARD_CHANGE` was silent for the same reason.
        _ => shared("ERR_GUILD_INTERNAL"),
    }
}

/// The message one `SMSG_GUILD_COMMAND_RESULT` prints, if any.
///
/// `result == 0` is the success side — the reference's `0x5e7550 test eax,eax` selects on exactly
/// that — and then the [`guild_command`] tag chooses the line. Otherwise the result code does,
/// except for `0x08`, whose two meanings are told apart by that same tag.
///
/// **Every `_S` key here is called with the name and every other with nothing** — the reference's
/// `add esp,8` / `add esp,4` split, 22 of 22 (`0x5e7520`). That
/// is a property a test can check against the key names, and one does.
pub(super) fn command_line(result: &GuildCommandResult) -> Option<UiError> {
    let name = result.name.as_str();
    let named = |key: &'static str| Some(UiError::s(key, name));
    if result.result == guild_command_error::PLAYER_NO_MORE_IN_GUILD {
        return match result.command {
            guild_command::CREATE => named("ERR_GUILD_CREATE_S"),
            guild_command::INVITE => named("ERR_GUILD_INVITE_S"),
            guild_command::QUIT => named("ERR_GUILD_QUIT_S"),
            guild_command::FOUNDER => named("ERR_GUILD_FOUNDER_S"),
            // 19 and 20 re-request the roster and print nothing; vmangos's own note on the rest of
            // `Typecommand` is that they "have no effect" here.
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
        // The one collision: `0x08` is ERR_GUILD_LEADER_LEAVE under a QUIT and
        // ERR_GUILD_PERMISSIONS under anything else (benilla-protocol's own note on
        // `guild_command_error::LEADER_LEAVE`), which is why the command tag has to survive the
        // trip from the wire to here.
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
        // 15 and 16 are silent; `UNK20` re-requests the roster for command 5 and prints nothing;
        // anything unknown says nothing.
        _ => None,
    }
}

/// `SMSG_GUILD_INVITE`'s notice line — the one the reference prints *beside* the popup
/// (`0x5e6f65 DisplayError(0x4f, inviter, guild)`; the popup is the separate
/// `GUILD_INVITE_REQUEST` fire at `0x5e6f53`).
pub(super) fn invite_line(inviter: &str, guild: &str) -> UiError {
    UiError::strings("ERR_INVITED_TO_GUILD_SS", &[inviter, guild])
}

/// `SMSG_GUILD_DECLINE` — our invitee said no (delivered to the inviter only).
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

    /// A promotion names the catalog key and hands it the packet's three strings in order — the
    /// thing a re-typed sentence could not carry, since `%s` slots that take *different* values
    /// are what a replace-all substitution gets wrong ("Tigole has promoted Tigole to Tigole.").
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

    /// **A short `strCount` is passed SHORT, never padded** (`0x5e745f`): 0 and >= 4 pass nothing
    /// at all, and 1–3 pass exactly what arrived. The visible consequence is the starved
    /// specifier, which `SStrPrintf` copies through — the reason this is a behaviour and not an
    /// implementation detail.
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

    /// The sign-on/sign-off lines are the friend-list ones, they take the name TWICE and ONCE, and
    /// they are the only two arms in the table with a display condition — the whole reason those
    /// two carry a guid.
    ///
    /// **The polarity flipped in 1589** and this test says so on purpose: the flag used to mean
    /// "ignored" (suppress) and now means "announce", because `0x5ae810` turned out to be a
    /// friends-list test, not the ignore check the transcription named.
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

    /// The three genuinely silent events — and, beside them, the two that were silent by MISTAKE.
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
        // `0x09` and every id past `0x0d` share the default arm, which pushes `0x69`. Both used to
        // return `None`; a message id the reference pushes is not a message we may drop.
        for event in [guild_event::TABARD_CHANGE, 0x0e, 0x77, 0xff] {
            let e = event_line(&notice(event, &["x"]), false).expect("the default arm prints");
            assert_eq!(e.key, "ERR_GUILD_INTERNAL", "{event:#04x}");
        }
    }

    /// Result `0x08` means two different things, and only the command tag beside it says which.
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

    /// Result `0` is the success side, and there the command tag picks the line.
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

    /// **The `_S` arity control, 22 of 22** — the reference's `add esp,8` at every `_S` key and
    /// `add esp,4` at every other (`0x5e7520`). It is checkable here because the key
    /// name carries the arity, and it is the one thing about this table that a wrong key cannot
    /// pass silently.
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

    /// **Every key this module names is a catalog row.** A key that is not one resolves to nothing
    /// or, worse, takes `Shown::keyed`'s fallback and turns a chat line red — which is exactly the
    /// class decision 2033 found sitting unnoticed for months. The surfaces are asserted with it,
    /// because two of these are NOT chat and the old text path sent all of them there.
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

        // The two name refusals are `kind 2` — the red `UI_ERROR_MESSAGE`, not chat. Every text
        // this module used to compose went to chat, so these two were on the wrong surface.
        for key in ["ERR_GUILD_NAME_INVALID", "ERR_GUILD_NAME_EXISTS_S"] {
            assert_eq!(by_key(key).map(|r| r.kind), Some(MsgKind::Error), "{key}");
        }
        // …and the rest of the guild family is chat, which is where they already were.
        assert_eq!(
            by_key("ERR_GUILD_PROMOTE_SSS").map(|r| r.kind),
            Some(MsgKind::Chat)
        );
    }

    /// The invite notice names the inviter first and the guild second.
    #[test]
    fn the_invite_notice_names_both() {
        let e = invite_line("Tigole", "Legacy of Steel");
        assert_eq!(e.key, "ERR_INVITED_TO_GUILD_SS");
        assert_eq!(strings(&e), ["Tigole", "Legacy of Steel"]);
        assert_eq!(decline_line("Kaplan").key, "ERR_GUILD_DECLINE_S");
    }
}
