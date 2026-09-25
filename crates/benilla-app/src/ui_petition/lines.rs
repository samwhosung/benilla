//! The charter session's **system lines** — the messages the engine composes, and *which of the
//! client's two message channels each one goes to*.
//!
//! The split is the part a re-implementation cannot guess, and we did guess it wrong first: by the
//! key names, `ERR_PETITION_SIGNED_S` and `ERR_PETITION_ALREADY_SIGNED` look like the same kind of
//! thing. They are not. A sweep of every row of the message catalog at `0xb4b498` (stride `0x14`,
//! field `+4` = kind, routed through the four-entry table at `0x496888`) gives:
//!
//! - the **four** `_S`/success ids `0x141`-`0x144` carry kind `0` → the shared chat chokepoint
//!   `0x49a870` with the row's own `+0x10` chat type, which is `0xa` → **`CHAT_MSG_SYSTEM`**, the
//!   same channel every `ERR_GUILD_*` line uses;
//! - the **five** refusals `0x145`-`0x149`, and `ERR_NO_GUILD_CHARTER` (`0x7c`), carry kind `2` →
//!   `UI_ERROR_MESSAGE` (`0xe0`), the **red UIErrorsFrame line**.
//!
//! That sweep is no longer this module's to carry: since decision 1770 the whole registry ships as
//! [`benilla_ui::messages`], and each line below asks it by key. The paragraphs above stay because
//! they record *why* the split is not guessable — the reason a lookup was worth building.
//!
//! Two more facts from the same sweep that shape the tables below:
//!
//! - **`PETITION_SIGN_NEED_MORE` (4) has no user-visible message on the SIGN path at all.** It
//!   falls into the default arm with `>= 6` and reaches only the debug console
//!   (`0x63cb50("Petition error")`). It *does* have one on the turn-in path. Same code, two
//!   packets, two behaviours.
//! - **A successful turn-in emits no text whatsoever** (`0x5ef166`). It fires
//!   `GUILD_REGISTRAR_CLOSED` and says nothing. This file first shipped an `ERR_GUILD_FOUNDER_S`
//!   line there, reasoned from the fact that vmangos never sends the guild-command result that
//!   would produce one; the binary settles it — founding a guild really is silent, and the player
//!   learns of it from the guild pane lighting up.
//!
//! Every one of these rows also carries a named sound cue (`+0xc = 0x44`): `0x142`/`0x143` play
//! `igPlayerInviteAccept` and `0x144`-`0x149` play `igPlayerInviteDecline`, while `0x141` has none.
//! Recorded here; wiring the cue is [`super`]'s job, not this module's.

use benilla_protocol::messages::petition_result;

use crate::ui_action::UiError;

/// One composed line: a **catalog key** and the arguments the reference pushes with it, resolved
/// at the sink against the player's own `GlobalStrings.lua`.
///
/// This file used to carry the key *and* a hand-typed copy of each sentence, and the copy was the
/// part that could rot: three of the five validator lines shipped as **paraphrases** — "Names may
/// not begin or end with a space." where the client says "Guild names cannot start or end with a
/// space.  Enter a new name." — because they were written from the key name rather than read from
/// the file. A key cannot paraphrase. The oracle test that caught those is gone with the text it
/// graded; what replaces it asserts that every key here IS a catalog row and that an `_S` key's
/// arity matches the fill, which is the thing text comparison could never check.
///
/// The key rides rather than a resolved [`benilla_ui::messages::MsgKind`] because the record
/// answers more than where the line goes — `ERR_PETITION_SIGNED` and its neighbours name a sound
/// cue too (`igPlayerInviteAccept`/`Decline`), and `crate::ui_action::Shown` reads all of it from
/// the one row at the sink.
pub(super) type Line = UiError;

/// The line one `SMSG_PETITION_SIGN_RESULTS` prints **when the signer is us** — the switch at
/// `0x5eeff5`'s "I signed" leg.
///
/// The owner's copy of the same packet is a different path entirely: it appends the signer and
/// emits `ERR_PETITION_SIGNED_S`, and never inspects the result at all ([`signed_by_other`]).
///
/// `NEED_MORE` and anything `>= 6` return `None` — the default arm reaches only the debug console.
pub(super) fn my_sign_line(result: u32) -> Option<Line> {
    Some(UiError::key(match result {
        petition_result::OK => "ERR_PETITION_SIGNED",
        petition_result::ALREADY_SIGNED => "ERR_PETITION_ALREADY_SIGNED",
        petition_result::ALREADY_IN_GUILD => "ERR_PETITION_IN_GUILD",
        petition_result::CANT_SIGN_OWN => "ERR_PETITION_CREATOR",
        petition_result::NOT_SERVER => "ERR_PETITION_NOT_SAME_SERVER",
        // `NEED_MORE` (4) and >= 6: the default arm, debug console only.
        _ => return None,
    }))
}

/// The line the **owner** gets when somebody else signs — `ERR_PETITION_SIGNED_S`, a chat line, and
/// emitted **only when the signer's name is already cached** (`0x4f42f6`); on a cache miss the
/// client increments its pending-name counter instead and says nothing until the name lands.
pub(super) fn signed_by_other(name: &str) -> Line {
    UiError::s("ERR_PETITION_SIGNED_S", name)
}

/// The line one `SMSG_TURN_IN_PETITION_RESULTS` prints. **Success prints nothing** — see the module
/// doc; it is not an omission here.
pub(super) fn turn_in_line(result: u32) -> Option<Line> {
    Some(UiError::key(match result {
        petition_result::ALREADY_IN_GUILD => "ERR_PETITION_IN_GUILD",
        petition_result::NEED_MORE => "ERR_PETITION_NOT_ENOUGH_SIGNATURES",
        _ => return None,
    }))
}

/// The line an inbound `MSG_PETITION_DECLINE` prints to the charter's owner — a chat line, and
/// **only if the declining player's name is already cached**. There is no query and no retry: an
/// uncached name means the owner is told nothing at all (`0x5ef12a`/`0x5ef139`).
pub(super) fn declined_line(name: &str) -> Line {
    UiError::s("ERR_PETITION_DECLINED_S", name)
}

/// The local echo for a charter we just offered — emitted **optimistically on the send**, with no
/// server confirmation (`0x4f48fa`). Nothing comes back on success; the *target* is the one the
/// server answers.
pub(super) fn offered_line(name: &str) -> Line {
    UiError::s("ERR_PETITION_OFFERED_S", name)
}

/// Offering a charter to yourself — guard 6 of `OfferPetition`'s eight (`0x4f4839`), which emits
/// `ERR_PETITION_CREATOR`, the same red line signing your own charter gets.
pub(super) fn self_offer_line() -> Line {
    UiError::key("ERR_PETITION_CREATOR")
}

/// The local refusal when `TurnInGuildCharter()`'s bag scan finds no charter — a **red** line
/// (`0x5ef49a` emits id `0x7c`, kind 2), and no packet is built.
pub(super) fn no_charter_line() -> Line {
    UiError::key("ERR_NO_GUILD_CHARTER")
}

/// The line for a name the client's own validator refused
/// ([`benilla_ui::script::validate_guild_name`]) — a red line.
///
/// **These five rows were recorded here as INFERRED** — the sweep that settled the nine petition
/// ids and `0x7c` never covered the `0x75`-`0x7b` band, so the kind was reasoned from the fact
/// that every refusal in the swept set is kind 2. The full catalog confirms all
/// five: the inference was right, and it is a lookup now rather than a reason.
///
/// An unknown key falls back to the generic invalid-name text rather than showing a raw key.
pub(super) fn name_refused_line(key: &str) -> Line {
    UiError::key(match key {
        "ERR_GUILD_ENTER_NAME" => "ERR_GUILD_ENTER_NAME",
        "ERR_GUILD_NAME_INVALID_SPACE" => "ERR_GUILD_NAME_INVALID_SPACE",
        "ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES" => "ERR_GUILD_NAME_NAME_CONSECUTIVE_SPACES",
        "ERR_GUILD_NAME_TOO_SHORT" => "ERR_GUILD_NAME_TOO_SHORT",
        // The validator can name a key `0x6c9b70` is not known to produce. Re-keying it to the
        // generic invalid-name row is what keeps a raw key off the screen; the match above is
        // what makes that a re-key rather than a pass-through.
        _ => "ERR_GUILD_NAME_INVALID",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::messages::MsgKind;

    /// A composed line's key and the surface its catalog row names — the join the sink makes.
    ///
    /// **The key, not the sentence.** `ERR_PETITION_CREATOR` and `ERR_PETITION_ALREADY_SIGNED`
    /// say different things in English, but plenty of neighbours do not: asserting displayed text
    /// passes exactly where two rows agree in enUS and diverge in every other locale, which is why
    /// decision 2045 puts the assertion on the identifier.
    fn shown(line: Line) -> (&'static str, MsgKind) {
        (line.key, benilla_ui::messages::kind_of(line.key))
    }

    /// **The channel split, which does not follow the key names.** Success to chat, refusal to the
    /// red frame — swept out of the message catalog, not guessed.
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
        // The name really does reach the fill — the half of a `_S` line that a key assertion
        // alone would not cover.
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

    /// **`NEED_MORE` is silent on the sign path and loud on the turn-in path** — one result code,
    /// two packets, two behaviours. Reading the enum and giving both packets the same table is the
    /// mistake this pins, and it is the shape the first version of this file had.
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

    /// A successful turn-in says **nothing**. The window closing is the whole of the feedback.
    #[test]
    fn a_successful_turn_in_prints_no_line_at_all() {
        assert_eq!(turn_in_line(petition_result::OK), None);
        // …and the codes the turn-in packet cannot carry stay silent rather than borrowing the
        // sign path's wording.
        assert_eq!(turn_in_line(petition_result::CANT_SIGN_OWN), None);
        assert_eq!(turn_in_line(petition_result::ALREADY_SIGNED), None);
    }

    /// **Every key this file names, checked against the shipped `GlobalStrings.lua` and against
    /// the catalog.** The oracle it replaces compared our *copy* of each sentence to the file's;
    /// with the copies gone, the failures still worth catching are a key that resolves to nothing
    /// (the line would silently not appear) and a key whose arity disagrees with the fill — an
    /// `_S` row handed no argument, or a plain row handed one. Neither is visible in English.
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

    /// The validator's keys pass through, and a key `0x6c9b70`'s uncarved arms could name
    /// degrades to the generic invalid-name row rather than surfacing a raw key to the player.
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
