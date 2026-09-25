//! **`/afk` and `/dnd` — the client-side command law**.
//!
//! The two commands look like a pair and are not one. `SendChatMessage 0x49f1e0` handles them
//! **asymmetrically**, and the reference's own string pool shows why: the DND keys live in the chat
//! translation unit, the AFK keys in `Player_C.h`'s.
//!
//! - **DND** (`0x49f3de`-`0x49f591`) is inline, reads `PLAYER_FLAGS` bit `0x4` **live**, and falls
//!   through to the generic `CMSG_MESSAGECHAT` send.
//! - **AFK** (`0x49f4f3`-`0x49f562`) reads the client-side mirror [`AfkMirror`] — *not* the
//!   descriptor bit — and returns, delegating to `CGPlayer_C::SetAFK 0x5eb740` /
//!   `ClearAFK 0x5eb830`, which build their own packets.
//!
//! **The echo is OPTIMISTIC, never descriptor-driven**: the print and the mirror write are
//! straight-line before the send. `0x5ee990`'s `test al,0xe` arm — the `PLAYER_FLAGS` delta arm
//! that carries AFK/DND/GM — calls only `0x6c78f0` (name invalidation) and `0x468550` between
//! `0x5ee9c0` and `0x5ee9f8`: no string resolve, no chat sink. So a client that waited for the
//! server's flag to come back before printing would be wrong, and would also print nothing at all
//! when the send is dropped.
//!
//! **All four lines are `CHAT_MSG_SYSTEM`** (`0x49a870` with `edx = 0xa`; `[0x8062e0 + 4*0xa]` =
//! 238, name slot `0xbe1550` ← `"CHAT_MSG_SYSTEM"`), whose yellow is the **engine's** own
//! (`.rdata 0x8049ae` = `ff ff 00`), not a FrameXML `ChatTypeInfo` colour. The incoming events
//! `CHAT_MSG_AFK`/`DND` (249/250) are a different thing entirely — those are *other* players'
//! auto-replies, and never these.

use bevy::prelude::*;

use super::event::{ChatEvent, ChatEventKind};
use super::feed::ChatLog;

/// **`[0xb6e5cc]` — the write-through optimistic mirror of the local player's `PLAYER_FLAGS` AFK
/// bit**, reconciled by the descriptor. Authoritative for the client's own toggle decision, never
/// for anyone else's state.
///
/// Eleven sites in the image reference it. Written optimistically at command time
/// (`SetAFK 0x5eb7ae` ← 1, `ClearAFK 0x5eb885` ← 0), reconciled from the descriptor on every local
/// `PLAYER_FLAGS` delta (`0x5ee9f2`), re-seeded at world enter (`0x4989c0`, which is **not** the
/// `/afk` toggle). Consumed by the overhead-name AFK tag as the own-player override, and by the
/// `/afk` toggle itself.
///
/// **`u32`, not `bool`, and never compared against 1.** The five writers store **three** truthy
/// values — `1` (`0x4989eb`, `0x5eb7ae`), `0` (`0x4989f7`, `0x5eb885`) and **`2`** (`0x5ee9f2`
/// stores `PLAYER_FLAGS & 2`). All six readers are pure non-zero tests, so the reference never
/// notices; a re-implementation that types this `bool` or tests `== 1` diverges at the first
/// descriptor-driven set. That is the hazard, and this type is the answer to it.
///
/// **There is no DND mirror** — no global is written on the DND path, which is why [`dnd_line`]
/// takes the live descriptor bit and [`afk_line`] takes this. The observable asymmetry is real and
/// faithful: a second `/dnd` typed before the server's update arrives re-marks, while a second
/// `/afk` in the same window clears.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AfkMirror(pub(crate) u32);

impl AfkMirror {
    /// The only question anything asks of it — every one of the reference's six readers is a
    /// non-zero test, so this is the whole API and `== 1` is never written anywhere.
    pub(crate) fn is_afk(self) -> bool {
        self.0 != 0
    }
}

/// What a `/afk` or `/dnd` resolves to: the line to print (if any), the mirror's new value (AFK
/// only), and the body to put on the wire.
///
/// A struct rather than three returns because the truth table's whole content is which of them move
/// together — notably `/afk M` **while already AFK**, which prints nothing and leaves the mirror
/// alone but still sends (`0x5eb76e` jumps past both the echo and the flag write, not past the
/// packet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AwayOutcome {
    /// The `CHAT_MSG_SYSTEM` line, or `None` for the one row that prints nothing.
    pub(crate) line: Option<String>,
    /// The mirror's value afterwards. `None` = unchanged (every DND row, and `/afk M` while AFK).
    pub(crate) mirror: Option<u32>,
    /// The message body the `CMSG_MESSAGECHAT` carries — **after** the client's own default
    /// substitution, which is why it is returned rather than reusing the caller's string.
    pub(crate) body: String,
}

/// Resolve `/afk <msg>` against the mirror — the AFK arm `0x49f4f3`-`0x49f562`.
///
/// | `msg` | mirror before | line | mirror after |
/// |---|---|---|---|
/// | `""` | clear | `MARKED_AFK_MESSAGE` % `DEFAULT_AFK_MESSAGE` | 1 |
/// | `""` | set | `CLEARED_AFK` | 0 |
/// | `M` | clear | `MARKED_AFK_MESSAGE` % `M` | 1 |
/// | `M` | set | *(nothing)* | unchanged |
///
/// **The default text is the CLIENT's and is substituted BEFORE the send** (`0x49f512`): the server
/// receives the literal `"Away from Keyboard"`, never an empty body, so vmangos stores that as the
/// auto-reply. `SetAFK` substitutes again for a NULL message (`0x5eb761`) — belt and braces in the
/// reference, one substitution here.
///
/// `MARKED_AFK_MESSAGE` carries **no** trailing period (`"You are now AFK: %s"`); its sibling
/// `MARKED_AFK` (the no-`%s` variant `GlobalStrings.lua` also defines) has **zero occurrences in
/// the image** and is dead — do not reach for it when the message is empty, because the default
/// substitution means it never is.
pub(crate) fn afk_line(
    msg: &str,
    mirror: AfkMirror,
    strings: &impl Fn(&str) -> Option<String>,
) -> AwayOutcome {
    let text = |key: &str| strings(key).unwrap_or_default();
    if mirror.is_afk() {
        if msg.is_empty() {
            // The toggle off. `ClearAFK 0x5eb830` prints CLEARED_AFK unformatted and sends an
            // EMPTY body — the packet still goes, it just carries nothing.
            AwayOutcome {
                line: Some(text("CLEARED_AFK")),
                mirror: Some(0),
                body: String::new(),
            }
        } else {
            // **Re-marking while AFK is silent** — `0x5eb76e` jumps past the echo AND the mirror
            // write, but not past the send: the server still learns the new auto-reply text. This
            // row is the one that most tempts a re-implementer into printing something.
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

/// Resolve `/dnd <msg>` against the **live** `PLAYER_FLAGS` bit `0x4` — the DND arm
/// `0x49f3de`-`0x49f591`.
///
/// | `msg` | DND before | line |
/// |---|---|---|
/// | `""` | clear | `MARKED_DND` % `DEFAULT_DND_MESSAGE` |
/// | `""` | set | `CLEARED_DND` |
/// | `M` | clear | `MARKED_DND` % `M` |
/// | `M` | set | `MARKED_DND` % `M` — **re-prints**, unlike AFK |
///
/// `MARKED_DND` is `"You are now DND: %s."` — **the period is inside the format string**, which is
/// why it must be filled rather than composed; writing `"You are now DND: " + msg + "."` here would
/// be right in enUS and wrong in every locale that moved it.
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

/// The implicit AFK clear every **other** chat send carries — `0x49f3c7`/`0x49f3d6`, gated on
/// `autoClearAFK` and on the mirror being set, and skipped for chat type `0x14` alone (which is
/// why `/dnd`, being type `0x15`, clears AFK first — that is the whole reason the reference's own
/// capture shows three lines for two typed commands).
///
/// Returns the `CLEARED_AFK` line when it fires; the caller also sends the empty `0x14` packet and
/// zeroes the mirror, then goes on to send the message's own packet.
///
/// **It runs before the refusal arms, not after** — `0x49f3c7`/`0x49f3d6` sit above `0x49f592`, so
/// a ghost typing `/say` clears AFK and *then* gets `ERR_CHAT_WHILE_DEAD`.
pub(crate) fn auto_clear_line(
    mirror: AfkMirror,
    auto_clear_afk: bool,
    strings: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    (auto_clear_afk && mirror.is_afk()).then(|| strings("CLEARED_AFK").unwrap_or_default())
}

/// Fill a one-`%s` GlobalString. `%%` is an escaped percent; a template with no `%s` comes back
/// unchanged, which is the honest answer for a locale that dropped the slot.
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

/// The mirror's own last-seen `PLAYER_FLAGS`, so the reconcile below can diff like the reference's
/// watcher does. `None` = no local player held (logged out, or not yet in the world).
#[derive(Resource, Default)]
pub(crate) struct AfkMirrorMemo(Option<u32>);

/// **Reconcile the mirror from our own descriptor** — `0x5ee990`'s arm 1, and the world-enter
/// reseed (`0x4989c0`), which are the only two things that ever correct an optimistic write.
///
/// Its own system rather than a branch in the unit feed because that is the reference's own shape:
/// the write lives in the `PLAYER_FLAGS` delta handler, keyed on its own diff.
///
/// **The `& 0xE` gate is load-bearing, and dropping it is a real bug rather than a simplification.**
/// Arm 1 runs only when AFK, DND or GM *changed* (`0x5ee9ba a8 0e test al,0xe`), and only then
/// writes `PLAYER_FLAGS & 2` (`0x5ee9ef`/`0x5ee9f2`). A reconcile that instead wrote on *any*
/// flags delta would clobber the optimistic `1` back to `0` the first time an unrelated bit moved
/// — walk into an inn while AFK and the resting bit alone would drop your `<AFK>` a round trip
/// early. The whole point of the mirror is that it is allowed to lead the descriptor (for exactly
/// one round trip), and this gate is what protects that window.
///
/// The value written is `flags & 0x2` — so **`2`, not `1`** — which is exactly why [`AfkMirror`]
/// is a `u32` tested for non-zero and never compared against `1`.
pub(crate) fn reconcile_afk_mirror(
    self_q: Query<&crate::net::ObjectStore, With<crate::net::SelfPlayer>>,
    mut mirror: ResMut<AfkMirror>,
    mut memo: ResMut<AfkMirrorMemo>,
) {
    let Some(store) = self_q.iter().next() else {
        // No local player: the world-exit half of the world-enter reseed. Forgetting the memo is
        // what makes the next login re-seed rather than diff against the last body's flags.
        memo.0 = None;
        return;
    };
    let flags = store.0.player_flags();
    match memo.0.replace(flags) {
        // World enter (`0x4989c0`): seed from the descriptor outright, no diff to make.
        None => mirror.0 = flags & 0x2,
        // The delta arm: only an AFK/DND/GM move reconciles.
        Some(prev) if (prev ^ flags) & 0xE != 0 => mirror.0 = flags & 0x2,
        Some(_) => {}
    }
}

/// The **four movement clears** — `autoClearAFK`'s other call sites:
/// `Jump 0x513d36`, forward/back `0x514e23`, strafe `0x514f0b`, keyboard-turn `0x514fca`. All four
/// share one idiom — `if (OBJECT_FIELD_TYPE & TYPEMASK_PLAYER) ClearAFK(obj, 0)` sitting right
/// after the same function's stand-up call — so they are one rule here, not four.
///
/// **The press EDGE, and the keyboard's**: these sit in the movement *emitters*, which run off the
/// command, not off displacement. So mouse-look turning does not clear AFK (it never reaches
/// `0x514f50`), and neither does being knocked, feared or driven by a spline — you have to press
/// something. `binds` is the binding dispatch, so this follows a rebound key and already carries
/// the typing gate (`WASD` typed into the chat box is not movement).
///
/// Jump is in the set here where it is deliberately **out** of the follow-cancel set
/// (`player::follow`'s `move_start`, whose six commands are these minus JUMP) — two different
/// reference tables that happen to overlap, not one shared idea.
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
    // The inline-array idiom `player::follow`'s `move_start` uses — its six, plus JUMP.
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
    // The same empty `0x14` the command path sends — `ClearAFK` builds its own packet.
    let _ = commands.0.send(crate::net::ClientCommand::Chat {
        kind: crate::net::ChatKind::Afk,
        target: None,
        text: String::new(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enUS strings, exactly as the reference install's `GlobalStrings.lua` ships them —
    /// **including `MARKED_DND`'s trailing period, which lives INSIDE the format string** and is
    /// the reason these are filled rather than composed.
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

    /// **The reference's truth table, transcribed.** Every row, including the three that a
    /// re-implementer gets wrong by symmetry.
    #[test]
    fn the_truth_table_row_for_row() {
        let afk = |msg: &str, m: u32| afk_line(msg, AfkMirror(m), &strings);
        let dnd = |msg: &str, is: bool| dnd_line(msg, is, &strings);

        // `/afk`, not AFK → mark, with the CLIENT's default text substituted before the send.
        assert_eq!(
            afk("", 0),
            AwayOutcome {
                line: Some("You are now AFK: Away from Keyboard".into()),
                mirror: Some(1),
                body: "Away from Keyboard".into(),
            },
            "the server receives the literal default, never an empty body"
        );
        // `/afk`, AFK → the toggle off. The packet still goes, carrying nothing.
        assert_eq!(
            afk("", 1),
            AwayOutcome {
                line: Some("You are no longer AFK.".into()),
                mirror: Some(0),
                body: String::new(),
            }
        );
        // `/afk M`, not AFK → mark with M.
        assert_eq!(
            afk("brb", 0),
            AwayOutcome {
                line: Some("You are now AFK: brb".into()),
                mirror: Some(1),
                body: "brb".into(),
            }
        );
        // **`/afk M` while ALREADY AFK prints NOTHING** — `0x5eb76e` jumps past the echo and the
        // mirror write, but not past the send. The row most likely to be "fixed" into a print.
        assert_eq!(
            afk("brb", 1),
            AwayOutcome {
                line: None,
                mirror: None,
                body: "brb".into(),
            },
            "silent re-mark: the server still learns the new auto-reply"
        );

        // `/dnd`, not DND → the period comes from the format string, not from us.
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
        // **The asymmetry.** `/dnd M` while DND RE-PRINTS, where `/afk M` while AFK is silent.
        // Not a bug on either side: AFK reads its optimistic mirror, DND reads the live descriptor
        // bit and has no mirror at all (`0x49f3f0`).
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

    /// **The director's capture, reproduced: THREE lines from TWO commands.**
    ///
    /// `/afk` then `/dnd` prints the mark, then the implicit AFK clear, then the DND mark — because
    /// `/dnd` is chat type `0x15` and the auto-clear skips only `0x14` (`0x49f4f6`). Anyone reading
    /// the reference screenshot as three typed commands would build the wrong thing.
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
        // …and only THEN the DND arm, reading a now-clear AFK state — the order the reference
        // runs them in (`0x49f3c7` sits above the type dispatch).
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

    /// `autoClearAFK` off is a TOTAL no-op — no echo, and (at the call site) no mirror write and no
    /// packet. And a clear never fires when there is nothing to clear.
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
        // **The descriptor-driven value is 2, not 1** (`0x5ee9f2`): the reconcile stores
        // `PLAYER_FLAGS & 2`. A mirror typed `bool`, or tested `== 1`, would answer "not AFK" here
        // and silently stop clearing after the server's first confirmation.
        assert!(
            auto_clear_line(AfkMirror(2), true, &strings).is_some(),
            "2 is truthy — every one of the reference's six readers is a non-zero test"
        );
        assert!(AfkMirror(2).is_afk());
    }

    /// A locale that dropped or moved the `%s` must not produce a mangled sentence, and `%%` is an
    /// escaped percent. The reference `snprintf`s the shipped string verbatim; so do we.
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

    /// An install with no GlobalStrings (CI, a bare checkout) yields empty lines rather than
    /// English literals — the same posture `global_string` takes everywhere else. The BODY still
    /// carries the message, so the wire half is unaffected by a missing string table.
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
