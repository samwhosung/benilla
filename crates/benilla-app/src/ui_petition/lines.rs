//! The charter session's system lines, by message-catalog key. The catalog (`0xb4b498`, in
//! [`benilla_ui::messages`]) sends the success ids `0x141`-`0x144` to chat as `CHAT_MSG_SYSTEM`
//! and the refusals `0x145`-`0x149` and `0x7c` to the red `UI_ERROR_MESSAGE` line, a split the
//! key names do not predict. The sink also plays the sound a row names (`0x142`-`0x149` do).

use benilla_protocol::messages::petition_result;

use crate::ui_action::UiError;

/// One composed line: a catalog key and the arguments the reference pushes with it, resolved at
/// the sink against the player's own `GlobalStrings.lua`. The key rides, not a resolved kind,
/// because the sink reads the row's sound too.
pub(super) type Line = UiError;

/// The line `SMSG_PETITION_SIGN_RESULTS` prints when we signed (the switch at `0x5eeff5`); the
/// owner's copy never reads the result ([`signed_by_other`]).
pub(super) fn my_sign_line(result: u32) -> Option<Line> {
    Some(UiError::key(match result {
        petition_result::OK => "ERR_PETITION_SIGNED",
        petition_result::ALREADY_SIGNED => "ERR_PETITION_ALREADY_SIGNED",
        petition_result::ALREADY_IN_GUILD => "ERR_PETITION_IN_GUILD",
        petition_result::CANT_SIGN_OWN => "ERR_PETITION_CREATOR",
        petition_result::NOT_SERVER => "ERR_PETITION_NOT_SAME_SERVER",
        // `NEED_MORE` (4) and >= 6: the default arm, debug console only (`0x63cb50`).
        _ => return None,
    }))
}

/// The owner's chat line when another player signs, only when the signer's name is already cached
/// (`0x4f42f6`); on a miss the client bumps its pending-name counter and says nothing.
pub(super) fn signed_by_other(name: &str) -> Line {
    UiError::s("ERR_PETITION_SIGNED_S", name)
}

/// The line `SMSG_TURN_IN_PETITION_RESULTS` prints; success prints nothing (`0x5ef166`).
pub(super) fn turn_in_line(result: u32) -> Option<Line> {
    Some(UiError::key(match result {
        petition_result::ALREADY_IN_GUILD => "ERR_PETITION_IN_GUILD",
        petition_result::NEED_MORE => "ERR_PETITION_NOT_ENOUGH_SIGNATURES",
        _ => return None,
    }))
}

/// The owner's chat line for an inbound `MSG_PETITION_DECLINE`, only if the decliner's name is
/// already cached, with no query or retry (`0x5ef12a`/`0x5ef139`).
pub(super) fn declined_line(name: &str) -> Line {
    UiError::s("ERR_PETITION_DECLINED_S", name)
}

/// The echo for a charter we offered, printed on the send without confirmation (`0x4f48fa`);
/// the server answers only the target.
pub(super) fn offered_line(name: &str) -> Line {
    UiError::s("ERR_PETITION_OFFERED_S", name)
}

/// Offering a charter to yourself: guard 6 of `OfferPetition`'s eight (`0x4f4839`), the same red
/// line signing your own charter gets.
pub(super) fn self_offer_line() -> Line {
    UiError::key("ERR_PETITION_CREATOR")
}

/// `TurnInGuildCharter()` found no charter in the bags: a red line and no packet (`0x5ef49a`
/// emits id `0x7c`, kind 2).
pub(super) fn no_charter_line() -> Line {
    UiError::key("ERR_NO_GUILD_CHARTER")
}

/// A name the client's own validator refused ([`benilla_ui::script::validate_guild_name`]), as a
/// red line.
pub(super) fn name_refused_line(key: &str) -> Line {
    UiError::key(match key {
        "ERR_GUILD_ENTER_NAME" => "ERR_GUILD_ENTER_NAME",
        "ERR_GUILD_NAME_INVALID_SPACE" => "ERR_GUILD_NAME_INVALID_SPACE",
        "ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES" => "ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES",
        "ERR_GUILD_NAME_TOO_SHORT" => "ERR_GUILD_NAME_TOO_SHORT",
        // A key `0x6c9b70` is not known to produce becomes the generic row, never a raw key.
        _ => "ERR_GUILD_NAME_INVALID",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::messages::MsgKind;

    /// A line's key and the surface its catalog row names, the join the sink makes. Asserting the
    /// key rather than the enUS text holds where two rows read alike in one locale only.
    fn shown(line: Line) -> (&'static str, MsgKind) {
        (line.key, benilla_ui::messages::kind_of(line.key))
    }

    #[test]
    fn successes_go_to_chat_and_refusals_go_to_the_red_line() {
        assert_eq!(
            my_sign_line(petition_result::OK).map(shown),
            Some(("ERR_PETITION_SIGNED", MsgKind::Chat))
        );
        assert_eq!(
            shown(signed_by_other("Bob")),
            ("ERR_PETITION_SIGNED_S", MsgKind::Chat)
        );
        assert_eq!(
            shown(declined_line("Bob")),
            ("ERR_PETITION_DECLINED_S", MsgKind::Chat)
        );
        assert_eq!(
            shown(offered_line("Bob")),
            ("ERR_PETITION_OFFERED_S", MsgKind::Chat)
        );
        // The name reaches the fill, which a key assertion alone does not cover.
        assert_eq!(offered_line("Bob").arg_s(), Some("Bob"));

        for (code, key) in [
            (
                petition_result::ALREADY_SIGNED,
                "ERR_PETITION_ALREADY_SIGNED",
            ),
            (petition_result::ALREADY_IN_GUILD, "ERR_PETITION_IN_GUILD"),
            (petition_result::CANT_SIGN_OWN, "ERR_PETITION_CREATOR"),
            (petition_result::NOT_SERVER, "ERR_PETITION_NOT_SAME_SERVER"),
        ] {
            assert_eq!(
                my_sign_line(code).map(shown),
                Some((key, MsgKind::Error)),
                "code {code} is a RED line"
            );
        }
        assert_eq!(
            shown(no_charter_line()),
            ("ERR_NO_GUILD_CHARTER", MsgKind::Error)
        );
    }

    #[test]
    fn need_more_is_silent_when_signing_and_loud_when_turning_in() {
        assert_eq!(
            my_sign_line(petition_result::NEED_MORE),
            None,
            "the sign switch's default arm reaches only the debug console"
        );
        assert_eq!(
            turn_in_line(petition_result::NEED_MORE).map(shown),
            Some(("ERR_PETITION_NOT_ENOUGH_SIGNATURES", MsgKind::Error))
        );
    }

    #[test]
    fn a_successful_turn_in_prints_no_line_at_all() {
        assert_eq!(turn_in_line(petition_result::OK), None);
        // Codes the turn-in packet cannot carry stay silent, not borrowing the sign path's lines.
        assert_eq!(turn_in_line(petition_result::CANT_SIGN_OWN), None);
        assert_eq!(turn_in_line(petition_result::ALREADY_SIGNED), None);
    }

    /// Against the shipped `GlobalStrings.lua` and the catalog: a key that resolves to nothing
    /// would silently not print, and a `%s` row must get its argument and a plain row none.
    #[test]
    fn every_key_resolves_and_its_arity_matches_the_fill() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let src = chain
            .read_file("Interface\\FrameXML\\GlobalStrings.lua")
            .expect("GlobalStrings.lua in the chain");
        let s = benilla_ui::script::UiScript::new().expect("VM");
        s.run(&String::from_utf8_lossy(&src)).expect("runs clean");

        let mut lines: Vec<Line> = vec![
            signed_by_other("Bob"),
            declined_line("Bob"),
            offered_line("Bob"),
            self_offer_line(),
            no_charter_line(),
        ];
        lines.extend((0..8).filter_map(my_sign_line));
        lines.extend((0..8).filter_map(turn_in_line));
        lines.extend(
            [
                "ERR_GUILD_ENTER_NAME",
                "ERR_GUILD_NAME_INVALID",
                "ERR_GUILD_NAME_INVALID_SPACE",
                "ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES",
                "ERR_GUILD_NAME_TOO_SHORT",
            ]
            .map(name_refused_line),
        );

        for line in lines {
            let text: String = s
                .lua()
                .globals()
                .get(line.key)
                .unwrap_or_else(|e| panic!("{} missing from GlobalStrings: {e}", line.key));
            assert!(!text.is_empty(), "{} resolves empty", line.key);
            assert!(
                benilla_ui::messages::by_key(line.key).is_some(),
                "{} is not a catalog row, so its surface and sound would be a guess",
                line.key
            );
            assert_eq!(
                text.contains("%s"),
                line.arg_s().is_some(),
                "{} vs its fill",
                line.key
            );
        }
    }

    #[test]
    fn refused_names_keep_their_key_and_degrade_safely() {
        assert_eq!(
            shown(name_refused_line("ERR_GUILD_ENTER_NAME")),
            ("ERR_GUILD_ENTER_NAME", MsgKind::Error)
        );
        assert_eq!(
            shown(name_refused_line("ERR_GUILD_NAME_INVALID_SPACE")),
            ("ERR_GUILD_NAME_INVALID_SPACE", MsgKind::Error)
        );
        assert_eq!(
            shown(name_refused_line("ERR_SOMETHING_UNCARVED")),
            ("ERR_GUILD_NAME_INVALID", MsgKind::Error),
            "never a raw key on screen"
        );
    }
}
