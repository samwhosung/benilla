//! **`SendAddonMessage`** — the addon-to-addon channel.
//!
//! ```lua
//! SendAddonMessage(prefix, message [, distribution])
//! ```
//!
//! The corpus's single most-wanted missing global (24 of 218 addons name it). It is how an addon
//! talks to *its own copy running on someone else's client*: oRA2 broadcasting a CTRA raid status,
//! KLHThreatMeter shipping a threat table, BigWigs syncing a boss timer.
//!
//! ## 1.12.1 has no addon opcode — the whole thing is one sentinel in the language field
//!
//! There is no `CMSG_ADDON_MESSAGE`. `SendAddonMessage` composes its two strings into ONE chat
//! line, sends it as an ordinary `CMSG_MESSAGECHAT` on an ordinary chat lane, and sets the
//! `language` field to `LANG_ADDON` (`0xFFFFFFFF`). That sentinel is the *only* thing separating
//! addon data from speech, on both sides of the wire.
//!
//! VERIFIED in `WoW.exe` (5875). The binding is `0x49f920`, body
//! `[0x49f920, 0x49fb2a)`:
//!
//! ```text
//! 49f951/49f958: lua_tostring(L,1)  -> prefix    (NULL -> "" at 0x49f96b-0x49f97a)
//! 49f95d/49f966: lua_tostring(L,2)  -> message   (NULL -> "" likewise)
//! 49f97f-49f987: BOTH empty -> luaL_error("Usage: SendAddonMessage(...)")
//! 49f9a2: push 0x844b5c            ; "%s\t%s"  (raw 25 73 09 25 73 00)
//! 49f9ad: push 0x800               ; 2048
//! 49f9b2: push eax                 ; dst = [ebp-0x81c]
//! 49f9b3: call 0x64a7f0            ; _snprintf(dst, 0x800, "%s\t%s", prefix, message)
//! 49fa0b/49fa15: lua_isstring(L,3) ; absent/non-string -> esi = 1 = PARTY (0x49fa06/0x49fa12)
//! 49fa3f: cmp esi,0x01 ; je        ; PARTY
//! 49fa44: cmp esi,0x02 ; je        ; RAID
//! 49fa49: cmp esi,0x03 ; je        ; GUILD
//! 49fa4e: cmp esi,0x5c ; je        ; BATTLEGROUND
//! 49fa53: push 0x844b44 ; call 0x6f4940   ; luaL_error("Unknown addon chat type")
//! 49facf: call 0x418190             ; u32 opcode 0x95 = CMSG_MESSAGECHAT
//! 49fad8: call 0x418190             ; u32 chat type
//! 49fab9: or  ebx,-0x1
//! 49fadd: push ebx
//! 49fae1: call 0x4181f0             ; u32 language = 0xFFFFFFFF
//! 49faf0: call 0x418430             ; CString message
//! ```
//!
//! Three consequences the shape forces, and each is a thing an implementation gets wrong:
//!
//! - **The prefix is not a wire field.** It is glued to the message with a literal TAB and the
//!   pair rides as one C-string. The receiver splits on the FIRST tab (`0x49a8d0`). Nothing on
//!   the wire knows a prefix exists.
//! - **There is no `target` argument.** The body fetches Lua indices `1`, `2` and `3` and no
//!   other — there is no arg-4 fetch anywhere in its 522 bytes. 1.12 has no whispered addon
//!   message; the modern 4-argument `SendAddonMessage(prefix, msg, "WHISPER", target)` does not
//!   exist here, and `"WHISPER"` is not even a legal distribution (below).
//! - **`_snprintf` with size `0x800` truncates silently at 2047 bytes + NUL** (`0x64a861`-
//!   `0x64a868`). No error, no tell.
//!
//! ## The distribution set is FOUR, and it is narrower than the server's
//!
//! [`AddonDistribution`] is the whole legal set — `PARTY`/`RAID`/`GUILD`/`BATTLEGROUND`. Two
//! independent mechanisms in the binary name the identical four, which is the cross-check that
//! settles it: the send-side whitelist above, and the *receive* side's remap table (`0x49aff4`,
//! 92 bytes = `00 01 02` then `04` x88 then `03`) feeding the 5-entry jump table `0x49afe0`, whose
//! four real arms push `"PARTY"`/`"RAID"`/`"GUILD"`/`"BATTLEGROUND"` and whose default pushes the
//! literal `"UNKNOWN"`.
//!
//! **vmangos is WIDER than the client** and that gap is deliberate, not a bug to close:
//! `WorldSession::IsLanguageAllowedForChatType` also permits `LANG_ADDON` on OFFICER,
//! RAID_LEADER, RAID_WARNING, BATTLEGROUND_LEADER and CHANNEL. A real client *accepts* addon
//! traffic inbound on all of those (the receive-side discriminator is language-only and reads no
//! type byte) while refusing to *send* on them. We match the client, not the server: the four.
//!
//! An unknown token and a known-but-not-whitelisted one (`"SAY"`, `"WHISPER"`, `"CHANNEL"`,
//! `"OFFICER"`, `"RAID_WARNING"` — all of which DO resolve in the shared 13-record type table at
//! `0x49f7a0`) converge on the **same** `luaL_error` at `0x49fa53`. That is why this module does
//! not model the 13-record table at all: its output is unobservable through this binding, because
//! both paths raise the identical error text. The note says so outright — *"a caller cannot tell a
//! typo from a rejection."*
//!
//! ## The seam
//!
//! [`super::chat_send`]'s shape: the verb validates, composes, and queues an [`AddonSend`]; the app
//! drains it ([`super::UiScript::take_addon_sends`]) into `ClientCommand::AddonMessage` and the
//! writer puts it on the wire. Plain data across the boundary — this crate holds no wire types —
//! but the distribution crosses as an **enum, not a token string**, so the app's map is total and
//! the "unknown type silently guessed into SAY" failure is impossible by construction rather than
//! by a second validation nobody remembers to write.
//!
//! ## The `|`-escape scan — carved, and it is a forward SKIP, not a flat scan
//!
//! `0x49f9bb`-`0x49fa04`, running on the **composed** buffer (cursor `0x49f9bd
//! lea ecx,[ebp-0x81c]`, byte-identical to the `_snprintf` destination `lea` at `0x49f9a7`) and
//! strictly **before** the type whitelist — `0x49fa06` is the sole predecessor of `0x49fa0b`, so a
//! payload with a bad escape *and* a bad distribution reports the escape and never fetches
//! argument 3. See [`validate_escapes`] for the loop; the two facts that make it non-obvious:
//!
//! - **`|c` skips PAST its closer.** `strstr` (`0x64b4f0`, case-**sensitive** — the binary has a
//!   folding twin at `0x64b540` and deliberately does not use it here) finds the first literal
//!   `"|r"` and the cursor becomes `lea esi,[eax+0x2]` (`0x49f9f4`), i.e. beyond it. That single
//!   `lea` is why a well-formed `|r` never reaches the loop head — under a flat scan its own `|`
//!   would be a `|` followed by `r` and every colour escape in the game would raise.
//! - **An unmatched `|c` is not an error.** It NUL-terminates in place at the opening `|`
//!   (`0x49fa6a`) and then `0x49fa6d jmp 0x49fa06` lands on the accept path — **the truncated text
//!   is transmitted**, with no error and no signal to the addon.
//!
//! Because the scan runs on the composed buffer, prefix and message are validated as **one
//! string**: a `|c` opened in the prefix can be closed by a `|r` in the message, and an unmatched
//! one in the prefix truncates the payload to a bare prefix with no tab at all.
//!
//! ## What is deliberately NOT here, named rather than left to be found
//!
//! - **The no-group silent return** (`0x49fa9f`-`0x49faab` -> `0x49fb21`): the reference drops an
//!   effective-PARTY send when `[0xbc6f48]`/`[0xbc6f4c]` are both zero, with no error and no
//!   packet. Those two are **`partyMemberGuid[0].lo/.hi` — element 0 of a FOUR-entry array**
//!   (stride 8, limit `0x20`; `Ui\PartyFrame.cpp`), not "the party GUID" as 1235 called them:
//!   slot 0 being non-zero is a sound "in a party" test only because `SMSG_GROUP_LIST` refills the
//!   roster lowest-slot-first.
//!   We do not implement it, and the reason is a window we have and the reference does
//!   not: its gate reads the live group GUID, ours would read [`super::party::PartyState`], which
//!   is `default()` between world entry and the first `SMSG_GROUP_LIST`. Implementing it would
//!   silently swallow a real broadcast in that window — the exact failure this verb exists to
//!   avoid. Not implementing it costs one `CMSG_MESSAGECHAT` that vmangos discards
//!   (`HandleChatMessageOpcode`'s `if (!group) return`), which is the same observable nothing.

use mlua::{Lua, Value};

use super::Model;

/// The **four** distributions `SendAddonMessage` will send on — the client's own hard whitelist at
/// `0x49fa3f`-`0x49fa4e`, corroborated by the receive side's remap table (module doc).
///
/// An enum rather than a token string on purpose: the binding is the only place a distribution can
/// be rejected, so everything downstream must be total. The wire type bytes it names are
/// `ChatMsg::CHAT_MSG_*` and live in `benilla_protocol::messages`; this crate carries no wire
/// types, so the app maps the variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddonDistribution {
    /// `PARTY` — wire chat type `0x01`. The **default** when the third argument is absent or not a
    /// string (`esi` pre-seeded `1` at `0x49fa06`/`0x49fa12`, only overwritten by a successful
    /// `lua_isstring`+lookup).
    Party,
    /// `RAID` — wire chat type `0x02`. Downgraded to [`Self::Party`] outside a raid; see
    /// [`AddonDistribution::effective`].
    Raid,
    /// `GUILD` — wire chat type `0x03`.
    Guild,
    /// `BATTLEGROUND` — wire chat type `0x5C`.
    Battleground,
}

impl AddonDistribution {
    /// Resolve a Lua distribution token. `None` = raise `"Unknown addon chat type"`, which is the
    /// binding's single answer for both an unrecognised token and a recognised-but-not-whitelisted
    /// one (module doc).
    ///
    /// Case-insensitive because the reference's own lookup is: the shared table at `0x49f7a0` is
    /// scanned with the case-insensitive compare `0x64a4c0` (`0x49f861`-`0x49f875`), so
    /// `SendAddonMessage("p", "m", "raid")` is a RAID send on a real client.
    pub fn from_token(token: &str) -> Option<Self> {
        // `eq_ignore_ascii_case` rather than an uppercased allocation: the four tokens are ASCII
        // and this runs on every addon broadcast.
        for (name, dist) in [
            ("PARTY", Self::Party),
            ("RAID", Self::Raid),
            ("GUILD", Self::Guild),
            ("BATTLEGROUND", Self::Battleground),
        ] {
            if token.eq_ignore_ascii_case(name) {
                return Some(dist);
            }
        }
        None
    }

    /// The token the receive side reports as `CHAT_MSG_ADDON`'s 3rd argument — the strings the four
    /// jump-table arms push (`0x8441b0`, `0x844600`, `0x8441a8`, `0x8445f0`).
    pub fn token(self) -> &'static str {
        match self {
            Self::Party => "PARTY",
            Self::Raid => "RAID",
            Self::Guild => "GUILD",
            Self::Battleground => "BATTLEGROUND",
        }
    }

    /// **RAID sent from outside a raid goes to PARTY**, silently — `0x49fa8b`-`0x49fa93` tests
    /// `[0xb713e0]` and falls into the PARTY arm when it is zero. Every other distribution passes
    /// through untouched.
    ///
    /// This is the same downgrade a player sees when `/raid` in a 5-man party lands in party chat,
    /// and vmangos is the reason it has to exist client-side: `HandleChatMessageOpcode`'s
    /// `CHAT_MSG_RAID` arm requires `group->IsRaidGroup()` and drops the message otherwise. Without
    /// it, oRA2 — which sends `SendAddonMessage("CTRA", msg, "RAID")` unconditionally
    /// (`Core.lua:563`) — would reach nobody in an ordinary 5-man group, which is most of the time
    /// it runs.
    ///
    /// `in_raid` is [`super::party::PartyState::raid`] being non-empty, that list being
    /// `GetNumRaidMembers()`'s own backing (empty outside a raid, and including the player inside
    /// one).
    ///
    /// **That is not an analogy — it is the same cell**. `[0xb713e0]` is not a
    /// flag: it is the raid member **count**, used as an unsigned loop bound over the 40-slot
    /// roster at `0xb712a8`, and the binding `GetNumRaidMembers` (`0x4bb530`, `Ui\RaidInfo.cpp`)
    /// returns that cell verbatim. So the reference's test `[0xb713e0] == 0` *is*
    /// `GetNumRaidMembers() == 0`, and reading our own `GetNumRaidMembers` backing is the
    /// mechanism rather than a stand-in for it. (1235 recorded this identity as corroborated but
    /// hedged; the hedge is now retired.)
    pub fn effective(self, in_raid: bool) -> Self {
        match self {
            Self::Raid if !in_raid => Self::Party,
            other => other,
        }
    }
}

/// One queued addon broadcast, drained by the app into the wire.
///
/// Plain data — [`super::chat_send::ChatSend`]'s twin. The prefix/message split is **already
/// gone** by the time one of these exists: the reference composes at the binding
/// (`_snprintf(dst, 0x800, "%s\t%s", …)` at `0x49f9b3`) and so do we, because the tab is the only
/// separator the wire has and nothing downstream may re-decide where it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddonSend {
    /// The composed payload — `prefix`, TAB, `message` — truncated to the reference's 2047 bytes.
    /// Sent verbatim as the `CMSG_MESSAGECHAT` message C-string; vmangos does not sanitise a
    /// `LANG_ADDON` line (`SanitizeChatMessage` returns early, `Handlers/ChatHandler.cpp:49`), so
    /// the tab survives to the far client's splitter.
    pub text: String,
    /// The lane, **after** the outside-a-raid downgrade ([`AddonDistribution::effective`]) — what
    /// the app should actually send on, not what the addon asked for.
    pub distribution: AddonDistribution,
}

/// `_snprintf(dst, 0x800, …)` writes at most 2047 bytes plus its NUL (`0x64a861`-`0x64a868`).
const ADDON_MESSAGE_CAP: usize = 0x800 - 1;

/// **The `|`-escape scan** (`0x49f9bb`-`0x49fa04`) — the reference's only text
/// validation, run on the already-composed payload and **before** the distribution is even
/// fetched.
///
/// Returns `Err` for an invalid escape (the caller raises the reference's own message); otherwise
/// `Ok`, having possibly **truncated `text` in place** at an unmatched `|c`, which is the
/// reference's silent leg — the shortened payload still goes out.
///
/// The loop, arm for arm (`esi` = the cursor, always pointing at a `|` found by `strchr`):
///
/// | at `|` the next byte is | reference | cursor |
/// |---|---|---|
/// | `\|` | accept the escaped pipe | `+2` (`0x49f9d7 add esi,0x2`) |
/// | `c` and a later `"\|r"` exists | accept the whole colour run | past the closer (`0x49f9f4 lea esi,[eax+0x2]`) |
/// | `c` and no `"\|r"` | **truncate here, then accept** | — (`0x49fa6a`, `0x49fa6d jmp 0x49fa06`) |
/// | anything else, incl. the NUL | `luaL_error` | — (`0x49fa6f`) |
///
/// Consequences that fall straight out of that table and are pinned by the tests, because every
/// one of them is a thing a reimplementation gets wrong:
///
/// - **`||` and `|c` are the ONLY accepted escape starts.** A bare `|r` raises, and so do `|H`,
///   `|h`, `|T`, `|t`, `|n` — and `|C`, because the `c` test is case-sensitive. Item links survive
///   only because they are *wrapped*: `|cffffffff|Hitem:…|h[x]|h|r` opens with `|c`, so the whole
///   run including every `|H`/`|h` is skipped to the closing `|r` without being examined.
/// - **Nothing between a `|c` and its first `|r` is validated.** No hex-digit check; a nested
///   `|c` is legal precisely because it is never looked at. This is a skip, not a nesting counter.
/// - **`|R` does not close `|c`** (case-sensitive `strstr`) — it truncates instead.
/// - **A lone trailing `|` raises**, because `strchr` will not match the NUL terminator
///   (`0x64a402` tests before `0x64a406` compares), so the byte after it is the NUL and the NUL is
///   not `|` or `c`. Pipe runs must therefore be even: `||||` is sent verbatim, `|||` raises.
fn validate_escapes(text: &mut String) -> Result<(), ()> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while let Some(off) = bytes[i..].iter().position(|&b| b == b'|') {
        let pipe = i + off;
        match bytes.get(pipe + 1) {
            // `||` — an escaped pipe. Past both bytes.
            Some(b'|') => i = pipe + 2,
            Some(b'c') => {
                // `strstr` from the `|c`'s own `|` for the literal two bytes `"|r"` (operand
                // `0x844538` = image bytes `7c 72 00`). `|c` cannot match `|r`, so the hit is
                // always a later closer.
                match bytes[pipe..]
                    .windows(2)
                    .position(|w| w == b"|r")
                    .map(|p| pipe + p)
                {
                    // Skip PAST the closer, so its own `|` is never fed back to the loop head.
                    Some(closer) => i = closer + 2,
                    // No closer: NUL-terminate at the opening `|` and accept what is left.
                    None => {
                        text.truncate(pipe);
                        return Ok(());
                    }
                }
            }
            // Anything else — including `None`, the NUL that ends a lone trailing `|`.
            _ => return Err(()),
        }
    }
    Ok(())
}

/// `lua_tostring` semantics + the reference's NULL replacement: strings and numbers coerce,
/// everything else (nil, boolean, table, function) becomes the shared empty string `0x882748`
/// (`0x49f96b`-`0x49f97a`). `Lua::coerce_string` **is** `lua_tostring`, so this is the mechanism
/// rather than an imitation of it.
fn lua_tostring(lua: &Lua, v: &Value) -> mlua::Result<String> {
    Ok(lua
        .coerce_string(v.clone())?
        .map(|s| s.to_string_lossy())
        .unwrap_or_default())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "SendAddonMessage",
        lua.create_function(
            |lua, (prefix, message, distribution): (Value, Value, Value)| {
                let prefix = lua_tostring(lua, &prefix)?;
                let message = lua_tostring(lua, &message)?;
                // BOTH empty is the usage error, and only both — there is no empty-prefix-only and
                // no empty-message-only check (`0x49f97f`-`0x49f987`). AceEvent-2.0 relies on it:
                // `SendAddonMessage("LOOT_OPENED", "", "RAID")` is 24 of the corpus's 35 call
                // sites (the same line, in 23 addons' embedded copies) and its message is
                // deliberately empty. An empty-message guard would break the largest row here.
                if prefix.is_empty() && message.is_empty() {
                    return Err(mlua::Error::RuntimeError(
                        r#"Usage: SendAddonMessage("prefix", "message" [,"type"])"#.into(),
                    ));
                }
                let mut text = format!("{prefix}\t{message}");
                // Truncate exactly where `_snprintf` does — by BYTES, since the reference is
                // writing a C buffer. `floor_char_boundary` is not stable, so walk back to the
                // nearest boundary: identical to the reference for ASCII (every corpus payload),
                // and the alternative is a `String` that is not UTF-8, which cannot exist.
                if text.len() > ADDON_MESSAGE_CAP {
                    let mut cut = ADDON_MESSAGE_CAP;
                    while cut > 0 && !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    text.truncate(cut);
                }
                // **The escape scan runs HERE — after the compose, before argument 3 is fetched**
                // (`0x49fa06` is the sole predecessor of `0x49fa0b`). So a call
                // that is wrong in both ways reports the ESCAPE, and a bad distribution beside a
                // bad escape is never even looked at. It may truncate `text` in place, which is
                // the reference's silent leg.
                if validate_escapes(&mut text).is_err() {
                    return Err(mlua::Error::RuntimeError(
                        "Invalid escape code in chat message".into(),
                    ));
                }
                // The third argument goes through `lua_isstring`, which is TRUE for a number too
                // (`0x49fa0b`/`0x49fa15`) — so `SendAddonMessage("p", "m", 1)` resolves the token
                // `"1"`, finds nothing, and raises, rather than being read as chat type 1. Absent
                // or non-string (nil, table, boolean) takes `0x49fa1c je 0x49fa9f`, which skips
                // the whitelist entirely with the pre-seeded `esi = 1` — the PARTY default.
                let asked = match lua.coerce_string(distribution)? {
                    Some(token) => AddonDistribution::from_token(&token.to_string_lossy())
                        .ok_or_else(|| {
                            mlua::Error::RuntimeError("Unknown addon chat type".into())
                        })?,
                    None => AddonDistribution::Party,
                };
                let in_raid = {
                    let model = lua.app_data_ref::<Model>().expect("model app_data");
                    !model.party.raid.is_empty()
                };
                lua.app_data_mut::<Model>()
                    .expect("model app_data")
                    .addon_sends
                    .push(AddonSend {
                        text,
                        distribution: asked.effective(in_raid),
                    });
                Ok(())
            },
        )?,
    )?;
    Ok(())
}

impl super::UiScript {
    /// Drain the broadcasts `SendAddonMessage` queued since the last call.
    pub fn take_addon_sends(&mut self) -> Vec<AddonSend> {
        std::mem::take(&mut self.model_mut().addon_sends)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::party::{PartyState, RaidMemberInfo};
    use crate::script::UiScript;

    /// The composition law: prefix, a literal TAB, message — one string, and the prefix is not a
    /// field. Plus the default distribution, which is PARTY and not SAY (this verb's default is
    /// not `SendChatMessage`'s).
    #[test]
    fn a_broadcast_is_prefix_tab_message_and_defaults_to_party() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("oRA", "hello")"#).unwrap();
        assert_eq!(
            s.take_addon_sends(),
            vec![AddonSend {
                text: "oRA\thello".into(),
                distribution: AddonDistribution::Party,
            }]
        );
        // Drained, not re-read.
        assert!(s.take_addon_sends().is_empty());
    }

    /// **The four, and only the four.** Every token the shared 13-record table at `0x49f7a0`
    /// resolves but the whitelist at `0x49fa3f`-`0x49fa4e` rejects must raise — SAY and WHISPER
    /// especially, since a client that accepted them would be inventing a 1.12 whispered addon
    /// message that does not exist. The error text is the reference's, and it is the SAME one an
    /// unknown token gets.
    #[test]
    fn the_distribution_set_is_exactly_party_raid_guild_battleground() {
        let mut s = UiScript::new().unwrap();
        // In a raid, so RAID is not downgraded and its own byte is what is under test.
        s.set_party(PartyState {
            raid: vec![RaidMemberInfo::default()],
            ..Default::default()
        });
        for (token, want) in [
            ("PARTY", AddonDistribution::Party),
            ("RAID", AddonDistribution::Raid),
            ("GUILD", AddonDistribution::Guild),
            ("BATTLEGROUND", AddonDistribution::Battleground),
            // Case-insensitive: the reference's lookup uses the case-insensitive compare 0x64a4c0.
            ("raid", AddonDistribution::Raid),
            ("Guild", AddonDistribution::Guild),
        ] {
            s.run(&format!(r#"SendAddonMessage("p", "m", "{token}")"#))
                .unwrap_or_else(|e| panic!("{token} must send: {e}"));
            let sent = s.take_addon_sends();
            assert_eq!(sent.len(), 1, "{token}");
            assert_eq!(sent[0].distribution, want, "{token}");
        }
        // Resolved by the shared table, refused by the whitelist — one error text for both classes.
        for token in [
            "SAY",
            "YELL",
            "WHISPER",
            "EMOTE",
            "OFFICER",
            "CHANNEL",
            "AFK",
            "DND",
            "RAID_WARNING",
            "RAID_LEADER",
            "BATTLEGROUND_LEADER",
            "NOT_A_TYPE",
            "",
        ] {
            let err = s
                .run(&format!(r#"SendAddonMessage("p", "m", "{token}")"#))
                .expect_err(&format!("{token} must be refused, not guessed"));
            assert!(
                err.to_string().contains("Unknown addon chat type"),
                "{token}: {err}"
            );
            assert!(
                s.take_addon_sends().is_empty(),
                "{token} must queue nothing"
            );
        }
    }

    /// A refused distribution sends **nothing** — the whole point of the enum seam. An addon that
    /// asked for a lane we cannot send on gets an error it can see, never a silent reroute onto
    /// SAY (which would put its serialized payload in front of every player in range).
    #[test]
    fn a_number_distribution_raises_rather_than_being_read_as_a_type_byte() {
        let mut s = UiScript::new().unwrap();
        // `lua_isstring` is true for a number, so the reference resolves the TOKEN "1" — which is
        // not in the type table — and raises. It does not read 1 as CHAT_MSG_PARTY.
        let err = s
            .run(r#"SendAddonMessage("p", "m", 1)"#)
            .expect_err("a numeric distribution must raise");
        assert!(err.to_string().contains("Unknown addon chat type"), "{err}");
        assert!(s.take_addon_sends().is_empty());
    }

    /// Non-string, non-number third argument is **absent**, not an error: `lua_isstring` fails and
    /// the pre-seeded PARTY default stands.
    #[test]
    fn a_non_string_distribution_takes_the_party_default() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("p", "m", nil)"#).unwrap();
        s.run(r#"SendAddonMessage("p", "m", {})"#).unwrap();
        let sent = s.take_addon_sends();
        assert_eq!(sent.len(), 2);
        assert!(sent
            .iter()
            .all(|a| a.distribution == AddonDistribution::Party));
    }

    /// **RAID outside a raid is PARTY.** The corpus's one live caller (oRA2 `Core.lua:563`) sends
    /// `"RAID"` unconditionally, so without this its broadcasts reach nobody in an ordinary 5-man
    /// group — vmangos drops a `CHAT_MSG_RAID` from a non-raid group.
    #[test]
    fn raid_outside_a_raid_is_downgraded_to_party() {
        let mut s = UiScript::new().unwrap();
        // Default PartyState: not in a raid.
        s.run(r#"SendAddonMessage("CTRA", "status", "RAID")"#)
            .unwrap();
        assert_eq!(
            s.take_addon_sends()[0].distribution,
            AddonDistribution::Party
        );
        // In a raid it stays RAID…
        s.set_party(PartyState {
            raid: vec![RaidMemberInfo::default()],
            ..Default::default()
        });
        s.run(r#"SendAddonMessage("CTRA", "status", "RAID")"#)
            .unwrap();
        assert_eq!(
            s.take_addon_sends()[0].distribution,
            AddonDistribution::Raid
        );
        // …and the downgrade is RAID's alone — GUILD and BATTLEGROUND never move.
        s.set_party(PartyState::default());
        for (token, want) in [
            ("GUILD", AddonDistribution::Guild),
            ("BATTLEGROUND", AddonDistribution::Battleground),
        ] {
            s.run(&format!(r#"SendAddonMessage("p", "m", "{token}")"#))
                .unwrap();
            assert_eq!(s.take_addon_sends()[0].distribution, want, "{token}");
        }
    }

    /// Both arguments empty is the usage error; **either one alone is not**. AceEvent-2.0 —
    /// literally every one of this verb's replicated corpus callers — sends an empty message with
    /// a real prefix, and erroring on it would break the row this verb was written for.
    #[test]
    fn only_both_empty_is_a_usage_error() {
        let mut s = UiScript::new().unwrap();
        let err = s
            .run(r#"SendAddonMessage("", "")"#)
            .expect_err("both empty must raise");
        assert!(err.to_string().contains("Usage: SendAddonMessage"), "{err}");
        assert!(s.take_addon_sends().is_empty());
        // AceEvent-2.0.lua's own line, verbatim — the empty message must go out.
        s.run(r#"SendAddonMessage("LOOT_OPENED", "", "RAID")"#)
            .unwrap();
        // A prefix-less payload is legal too; the receiver reads prefix "" and message "m".
        s.run(r#"SendAddonMessage("", "m")"#).unwrap();
        let sent = s.take_addon_sends();
        assert_eq!(sent[0].text, "LOOT_OPENED\t");
        assert_eq!(sent[1].text, "\tm");
    }

    /// `lua_tostring` coercion, and the NULL -> "" replacement at `0x49f96b`-`0x49f97a`: a number
    /// coerces, a table does not and becomes empty. `SendAddonMessage({}, {})` is therefore the
    /// usage error, reached the same way `("", "")` reaches it.
    #[test]
    fn arguments_coerce_like_lua_tostring_and_nil_becomes_empty() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("v", 314)"#).unwrap();
        assert_eq!(s.take_addon_sends()[0].text, "v\t314");
        s.run(r#"SendAddonMessage(42, "m")"#).unwrap();
        assert_eq!(s.take_addon_sends()[0].text, "42\tm");
        // Non-coercible -> "" on both sides -> the usage error, not a Rust type error.
        let err = s
            .run(r#"SendAddonMessage({}, {})"#)
            .expect_err("two tables coerce to two empty strings");
        assert!(err.to_string().contains("Usage: SendAddonMessage"), "{err}");
    }

    /// **The first tab is the split, so a tab inside the prefix is a payload bug the reference also
    /// has** — no sanitisation of `0x09` exists anywhere in the binding. Pinned so nobody "fixes"
    /// it: the receiver would split at the prefix's own tab and read a truncated prefix.
    #[test]
    fn a_tab_inside_the_prefix_is_passed_through_unsanitised() {
        let mut s = UiScript::new().unwrap();
        s.run("SendAddonMessage(\"pre\\tfix\", \"body\")").unwrap();
        assert_eq!(s.take_addon_sends()[0].text, "pre\tfix\tbody");
    }

    /// Silent truncation at 2047 bytes + the implicit NUL — `_snprintf`'s `0x800`, not the message
    /// length and not the prefix length. No error: the addon is never told.
    #[test]
    fn the_payload_truncates_silently_at_2047_bytes() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("p", string.rep("x", 4000))"#)
            .unwrap();
        let sent = s.take_addon_sends();
        assert_eq!(sent[0].text.len(), 2047);
        // The tab survives the cut — it is at byte 1, far inside the cap.
        assert!(sent[0].text.starts_with("p\tx"));
        // A payload that exactly fills the buffer is NOT truncated.
        s.run(r#"SendAddonMessage("p", string.rep("y", 2045))"#)
            .unwrap();
        assert_eq!(s.take_addon_sends()[0].text.len(), 2047);
    }

    /// **The escape scan is a forward SKIP, and a well-formed colour run must survive it.** This
    /// is the test that would have caught the naive flat scan: under one, the `|` of every `|r`
    /// terminator is a `|` followed by `r` and *every* colour escape in the game raises.
    #[test]
    fn a_well_formed_colour_run_passes_and_its_closer_is_skipped_not_scanned() {
        let mut s = UiScript::new().unwrap();
        for payload in [
            "|cffff0000red|r",
            "before |cffff0000red|r after",
            // Nested opens are legal precisely because nothing between `|c` and the first `|r` is
            // examined — a skip, not a nesting counter.
            "|cffAA0000 a |cff00BB00 b |r",
            // An item link survives only because it is WRAPPED: the `|H`/`|h` inside the colour
            // run are never looked at.
            "|cffa335ee|Hitem:12345:0:0:0|h[Thunderfury]|h|r",
            // Escaped pipes, in even runs.
            "a || b",
            "||||",
        ] {
            s.run(&format!(
                "SendAddonMessage(\"P\", {:?}, \"GUILD\")",
                payload
            ))
            .unwrap_or_else(|e| panic!("{payload:?} must send: {e}"));
            let sent = s.take_addon_sends();
            assert_eq!(sent.len(), 1, "{payload:?}");
            assert_eq!(
                sent[0].text,
                format!("P\t{payload}"),
                "{payload:?} verbatim"
            );
        }
    }

    /// `||` and `|c` are the ONLY accepted starts — everything else raises with the reference's own
    /// text, including a **bare `|r`**, the link tokens outside a colour run, the case-flipped
    /// `|C`, and a lone trailing `|` (whose next byte is the NUL, which `strchr` never matches).
    #[test]
    fn every_other_escape_start_raises_including_a_bare_closer_and_a_trailing_pipe() {
        let mut s = UiScript::new().unwrap();
        for payload in [
            "|r",                    // a closer with nothing open
            "|Hitem:1:0:0:0|h[x]|h", // link tokens NOT wrapped in a colour run
            "|h",
            "|T",
            "|t",
            "|n",
            "|C",         // |C is not |c — the test is case-sensitive
            "trailing |", // next byte is the NUL
            "|||",        // odd pipe run: the third one's next byte is the NUL
        ] {
            let err = s
                .run(&format!(
                    "SendAddonMessage(\"P\", {:?}, \"GUILD\")",
                    payload
                ))
                .expect_err(&format!("{payload:?} must raise"));
            assert!(
                err.to_string()
                    .contains("Invalid escape code in chat message"),
                "{payload:?}: {err}"
            );
            assert!(
                s.take_addon_sends().is_empty(),
                "{payload:?} queues nothing"
            );
        }
    }

    /// **An unmatched `|c` is NOT an error — it truncates in place and the short payload is still
    /// sent** (`0x49fa6a` then `0x49fa6d jmp 0x49fa06`, the accept path). Silent, exactly as the
    /// reference is: the addon is never told its message was cut.
    #[test]
    fn an_unmatched_colour_open_truncates_in_place_and_still_sends() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("P", "keep|cff00ff00dropped", "GUILD")"#)
            .unwrap();
        assert_eq!(s.take_addon_sends()[0].text, "P\tkeep");
        // `|R` does not close `|c` — the strstr is case-SENSITIVE, so this truncates too.
        s.run(r#"SendAddonMessage("P", "keep|cff00ff00x|Ry", "GUILD")"#)
            .unwrap();
        assert_eq!(s.take_addon_sends()[0].text, "P\tkeep");
        // The scan runs on the COMPOSED buffer, so an unmatched open in the PREFIX truncates the
        // payload to a bare prefix — no tab, no message at all.
        s.run(r#"SendAddonMessage("pre|cffAABBCCfix", "body", "GUILD")"#)
            .unwrap();
        assert_eq!(s.take_addon_sends()[0].text, "pre");
    }

    /// **Prefix and message are validated as ONE string**, because the scan runs after the
    /// compose — so a colour opened in the prefix is legally closed by the message.
    #[test]
    fn a_colour_run_may_span_the_tab_because_the_scan_is_on_the_composed_buffer() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("|cffff0000pre", "post|r", "GUILD")"#)
            .unwrap();
        assert_eq!(s.take_addon_sends()[0].text, "|cffff0000pre\tpost|r");
    }

    /// **Ordering: the escape check precedes the distribution fetch.** A call that is wrong in
    /// both ways reports the ESCAPE — `0x49fa06` is the sole predecessor of `0x49fa0b`, so
    /// argument 3 is never even read. (1235 shipped these the other way round; 1236 corrects it.)
    #[test]
    fn a_bad_escape_beats_a_bad_distribution() {
        let s = UiScript::new().unwrap();
        let err = s
            .run(r#"SendAddonMessage("P", "|z", "TOTALLY_BOGUS")"#)
            .expect_err("must raise");
        assert!(
            err.to_string()
                .contains("Invalid escape code in chat message"),
            "the escape must win, not the type: {err}"
        );
        // But an UNMATCHED `|c` is not an error, so there the bad type does surface — the exact
        // asymmetry the two legs of the scan produce.
        let err = s
            .run(r#"SendAddonMessage("P", "|cff000000x", "TOTALLY_BOGUS")"#)
            .expect_err("must raise");
        assert!(err.to_string().contains("Unknown addon chat type"), "{err}");
    }

    /// The distribution tokens round-trip to the strings the RECEIVE side reports as
    /// `CHAT_MSG_ADDON`'s third argument (the four jump-table arms at `0x49a918`/`91f`/`926`/`92d`).
    #[test]
    fn the_tokens_are_the_receive_sides_own_strings() {
        for (dist, token) in [
            (AddonDistribution::Party, "PARTY"),
            (AddonDistribution::Raid, "RAID"),
            (AddonDistribution::Guild, "GUILD"),
            (AddonDistribution::Battleground, "BATTLEGROUND"),
        ] {
            assert_eq!(dist.token(), token);
            assert_eq!(AddonDistribution::from_token(token), Some(dist));
        }
    }
}
