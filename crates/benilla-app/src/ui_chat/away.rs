//! `/afk` and `/dnd`, which the reference's `SendChatMessage` (`0x49f1e0`) handles differently.
//! DND (`0x49f3de`-`0x49f591`) reads `PLAYER_FLAGS` bit `0x4` live and falls through to the plain
//! `CMSG_MESSAGECHAT` send. AFK (`0x49f4f3`-`0x49f562`) reads the client-side [`AfkMirror`] and
//! hands off to `CGPlayer_C::SetAFK` (`0x5eb740`) or `ClearAFK` (`0x5eb830`), which send their
//! own packets. Both echo before the send, never from the descriptor: the `PLAYER_FLAGS` delta arm
//! (`0x5ee990`) prints nothing. The four lines are `CHAT_MSG_SYSTEM` (`0x49a870`, type `0xA`);
//! the `CHAT_MSG_AFK`/`DND` events are other players' auto-replies.

use bevy::prelude::*;

use super::event::{ChatEvent, ChatEventKind};
use super::feed::ChatLog;

/// The reference's optimistic mirror of our `PLAYER_FLAGS` AFK bit (`[0xb6e5cc]`): written at
/// command time (`0x5eb7ae`, `0x5eb885`), reconciled from the descriptor (`0x5ee9f2`), reseeded
/// at world enter (`0x4989c0`). A `u32` tested for non-zero: the reconcile stores
/// `PLAYER_FLAGS & 2`, so a set mirror can read 2. DND has no mirror, so a second `/dnd` before
/// the server's update re-marks where a second `/afk` clears.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AfkMirror(pub(crate) u32);

impl AfkMirror {
    /// Non-zero, as all six of the reference's readers test it.
    pub(crate) fn is_afk(self) -> bool {
        self.0 != 0
    }
}

/// What a `/afk` or `/dnd` resolves to: the line to print, the mirror's new value, the wire body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AwayOutcome {
    /// The `CHAT_MSG_SYSTEM` line, or `None` for the one row that prints nothing.
    pub(crate) line: Option<String>,
    /// The mirror's value afterwards. `None` = unchanged (every DND row, and `/afk M` while AFK).
    pub(crate) mirror: Option<u32>,
    /// The `CMSG_MESSAGECHAT` body, after the client's own default-message substitution.
    pub(crate) body: String,
}

/// `/afk <msg>`, the AFK arm (`0x49f4f3`-`0x49f562`). The default text is substituted before the
/// send (`0x49f512`), so the server stores `"Away from Keyboard"`, never an empty body, and the
/// no-`%s` `MARKED_AFK` goes unused.
pub(crate) fn afk_line(
    msg: &str,
    mirror: AfkMirror,
    strings: &impl Fn(&str) -> Option<String>,
) -> AwayOutcome {
    let text = |key: &str| strings(key).unwrap_or_default();
    if mirror.is_afk() {
        if msg.is_empty() {
            // `ClearAFK` prints `CLEARED_AFK` unformatted and still sends, with an empty body.
            AwayOutcome {
                line: Some(text("CLEARED_AFK")),
                mirror: Some(0),
                body: String::new(),
            }
        } else {
            // Re-marking while AFK prints nothing (`0x5eb76e`), but the new text is still sent.
            AwayOutcome {
                line: None,
                mirror: None,
                body: msg.to_string(),
            }
        }
    } else {
        let body = if msg.is_empty() {
            text("DEFAULT_AFK_MESSAGE")
        } else {
            msg.to_string()
        };
        AwayOutcome {
            line: Some(fill_one(&text("MARKED_AFK_MESSAGE"), &body)),
            mirror: Some(1),
            body,
        }
    }
}

/// `/dnd <msg>` against the live `PLAYER_FLAGS` bit `0x4`, the DND arm (`0x49f3de`-`0x49f591`);
/// unlike `/afk`, a repeat with a message re-prints. `MARKED_DND` carries its period inside the
/// format string, so it is filled, never composed.
pub(crate) fn dnd_line(
    msg: &str,
    is_dnd: bool,
    strings: &impl Fn(&str) -> Option<String>,
) -> AwayOutcome {
    let text = |key: &str| strings(key).unwrap_or_default();
    if is_dnd && msg.is_empty() {
        return AwayOutcome {
            line: Some(text("CLEARED_DND")),
            mirror: None,
            body: String::new(),
        };
    }
    let body = if msg.is_empty() {
        text("DEFAULT_DND_MESSAGE")
    } else {
        msg.to_string()
    };
    AwayOutcome {
        line: Some(fill_one(&text("MARKED_DND"), &body)),
        mirror: None,
        body,
    }
}

/// The implicit AFK clear on every chat send but type `0x14` (`0x49f3c7`/`0x49f3d6`), gated on
/// `autoClearAFK`, so `/dnd` (`0x15`) clears AFK first. The caller sends the empty `0x14` packet
/// and zeroes the mirror. It runs before the refusal arms (`0x49f592`): a ghost's `/say` clears
/// AFK, then gets `ERR_CHAT_WHILE_DEAD`.
pub(crate) fn auto_clear_line(
    mirror: AfkMirror,
    auto_clear_afk: bool,
    strings: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    (auto_clear_afk && mirror.is_afk()).then(|| strings("CLEARED_AFK").unwrap_or_default())
}

/// Fill the first `%s` of a GlobalString, which the reference `snprintf`s; `%%` is a percent.
fn fill_one(template: &str, arg: &str) -> String {
    let mut out = String::with_capacity(template.len() + arg.len());
    let mut chars = template.chars().peekable();
    let mut used = false;
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('s') if !used => {
                out.push_str(arg);
                used = true;
            }
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Push one of the four lines into the chat log as `CHAT_MSG_SYSTEM`.
pub(crate) fn push_system(chat: &mut ChatLog, line: String) {
    chat.push_event(ChatEvent::text_only(ChatEventKind::System, line));
}

/// The last `PLAYER_FLAGS` the reconcile saw; `None` with no local player in the world.
#[derive(Resource, Default)]
pub(crate) struct AfkMirrorMemo(Option<u32>);

/// Reconcile the mirror from our descriptor: the `PLAYER_FLAGS` delta arm (`0x5ee990`) and the
/// world-enter reseed (`0x4989c0`). The delta arm writes `flags & 2` only when AFK, DND or GM
/// moved (`0x5ee9ba test al,0xe`); writing on any delta would let an unrelated bit, such as
/// resting, clear an optimistic AFK a round trip early.
pub(crate) fn reconcile_afk_mirror(
    self_q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    mut mirror: ResMut<AfkMirror>,
    mut memo: ResMut<AfkMirrorMemo>,
) {
    let Some(store) = self_q.iter().next() else {
        // Forgetting the memo makes the next world enter reseed rather than diff.
        memo.0 = None;
        return;
    };
    let flags = store.0.player_flags();
    match memo.0.replace(flags) {
        // World enter (`0x4989c0`): seed outright.
        None => mirror.0 = flags & 0x2,
        // The delta arm: only an AFK/DND/GM move reconciles.
        Some(prev) if (prev ^ flags) & 0xE != 0 => mirror.0 = flags & 0x2,
        Some(_) => {}
    }
}

/// The movement AFK clears: jump (`0x513d36`), forward/back (`0x514e23`), strafe (`0x514f0b`)
/// and keyboard turn (`0x514fca`). They sit in the movement emitters, so only a key press
/// clears; mouse-look turning never reaches `0x514f50`, and knockback, fear and splines press
/// nothing. Jump is here but not in `player::follow`'s cancel set: two separate reference tables.
pub(crate) fn movement_clears_afk(
    binds: Res<crate::bindings::BindingsState>,
    cvars: Res<crate::cvars::Cvars>,
    script: Option<NonSendMut<benilla_ui::script::UiScript>>,
    commands: Res<crate::net::NetCommands>,
    mut chat: ResMut<ChatLog>,
    mut mirror: ResMut<AfkMirror>,
) {
    use crate::bindings::cmd;
    if !mirror.is_afk() || cvars.flag("autoClearAFK") == Some(false) {
        return;
    }
    let moved = [
        cmd::JUMP,
        cmd::MOVE_FORWARD,
        cmd::MOVE_BACKWARD,
        cmd::STRAFE_LEFT,
        cmd::STRAFE_RIGHT,
        cmd::TURN_LEFT,
        cmd::TURN_RIGHT,
    ]
    .iter()
    .any(|&c| binds.just_pressed(c));
    if !moved {
        return;
    }
    let line = script
        .and_then(|s| super::combat::global_string(&s, "CLEARED_AFK"))
        .unwrap_or_default();
    push_system(&mut chat, line);
    mirror.0 = 0;
    // `ClearAFK`'s own packet: the empty `0x14` the command path sends.
    let _ = commands.0.send(crate::net::ClientCommand::Chat {
        kind: crate::net::ChatKind::Afk,
        target: None,
        text: String::new(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enUS `GlobalStrings.lua` values; `MARKED_DND` carries its period inside the format.
    fn strings(key: &str) -> Option<String> {
        Some(
            match key {
                "MARKED_AFK_MESSAGE" => "You are now AFK: %s",
                "CLEARED_AFK" => "You are no longer AFK.",
                "MARKED_DND" => "You are now DND: %s.",
                "CLEARED_DND" => "You are no longer marked DND.",
                "DEFAULT_AFK_MESSAGE" => "Away from Keyboard",
                "DEFAULT_DND_MESSAGE" => "Do not Disturb",
                _ => return None,
            }
            .to_string(),
        )
    }

    #[test]
    fn the_truth_table_row_for_row() {
        let afk = |msg: &str, m: u32| afk_line(msg, AfkMirror(m), &strings);
        let dnd = |msg: &str, is: bool| dnd_line(msg, is, &strings);

        assert_eq!(
            afk("", 0),
            AwayOutcome {
                line: Some("You are now AFK: Away from Keyboard".into()),
                mirror: Some(1),
                body: "Away from Keyboard".into(),
            },
            "the server receives the literal default, never an empty body"
        );
        assert_eq!(
            afk("", 1),
            AwayOutcome {
                line: Some("You are no longer AFK.".into()),
                mirror: Some(0),
                body: String::new(),
            }
        );
        assert_eq!(
            afk("brb", 0),
            AwayOutcome {
                line: Some("You are now AFK: brb".into()),
                mirror: Some(1),
                body: "brb".into(),
            }
        );
        assert_eq!(
            afk("brb", 1),
            AwayOutcome {
                line: None,
                mirror: None,
                body: "brb".into(),
            },
            "silent re-mark: the server still learns the new auto-reply"
        );

        assert_eq!(
            dnd("", false),
            AwayOutcome {
                line: Some("You are now DND: Do not Disturb.".into()),
                mirror: None,
                body: "Do not Disturb".into(),
            }
        );
        assert_eq!(
            dnd("", true),
            AwayOutcome {
                line: Some("You are no longer marked DND.".into()),
                mirror: None,
                body: String::new(),
            }
        );
        assert_eq!(
            dnd("busy", false),
            AwayOutcome {
                line: Some("You are now DND: busy.".into()),
                mirror: None,
                body: "busy".into(),
            }
        );
        // `/dnd M` while DND re-prints: DND reads the live bit and has no mirror (`0x49f3f0`).
        assert_eq!(
            dnd("busy", true),
            AwayOutcome {
                line: Some("You are now DND: busy.".into()),
                mirror: None,
                body: "busy".into(),
            },
            "DND re-marks where AFK stays silent — the reference's own asymmetry"
        );
    }

    /// `/afk` then `/dnd` prints three lines: the auto-clear skips only type `0x14` (`0x49f4f6`),
    /// so `/dnd` (`0x15`) clears AFK first.
    #[test]
    fn afk_then_dnd_prints_three_lines() {
        let mut mirror = AfkMirror::default();
        let mut out = Vec::new();

        let first = afk_line("", mirror, &strings);
        out.push(first.line.clone().unwrap());
        mirror.0 = first.mirror.unwrap();

        // `/dnd` takes the generic path first: the clear fires because the mirror is set.
        let cleared = auto_clear_line(mirror, true, &strings).expect("the implicit clear fires");
        out.push(cleared);
        mirror.0 = 0;
        // Then the DND arm: `0x49f3c7` sits above the type dispatch.
        assert!(!mirror.is_afk(), "the clear really cleared it");
        out.push(dnd_line("", false, &strings).line.unwrap());

        assert_eq!(
            out,
            vec![
                "You are now AFK: Away from Keyboard",
                "You are no longer AFK.",
                "You are now DND: Do not Disturb.",
            ],
            "the reference capture, line for line"
        );
    }

    #[test]
    fn the_auto_clear_is_gated_both_ways() {
        assert!(
            auto_clear_line(AfkMirror(1), false, &strings).is_none(),
            "cvar off"
        );
        assert!(
            auto_clear_line(AfkMirror(0), true, &strings).is_none(),
            "not afk"
        );
        assert!(auto_clear_line(AfkMirror(1), true, &strings).is_some());
        // The reconcile stores `PLAYER_FLAGS & 2` (`0x5ee9f2`), so a set mirror can read 2.
        assert!(
            auto_clear_line(AfkMirror(2), true, &strings).is_some(),
            "2 is truthy — every one of the reference's six readers is a non-zero test"
        );
        assert!(AfkMirror(2).is_afk());
    }

    #[test]
    fn the_fill_handles_the_shapes_globalstrings_can_carry() {
        assert_eq!(
            fill_one("You are now AFK: %s", "brb"),
            "You are now AFK: brb"
        );
        assert_eq!(fill_one("%s.", "x"), "x.");
        assert_eq!(fill_one("no slot", "x"), "no slot");
        assert_eq!(fill_one("100%% sure", "x"), "100% sure");
        assert_eq!(
            fill_one("%s and %s", "x"),
            "x and %s",
            "only the first slot is ours"
        );
    }

    #[test]
    fn a_missing_string_table_does_not_invent_english() {
        let none = |_: &str| None;
        let out = afk_line("brb", AfkMirror(0), &none);
        assert_eq!(out.line.as_deref(), Some(""));
        assert_eq!(
            out.body, "brb",
            "the wire half never depends on the strings"
        );
    }
}
