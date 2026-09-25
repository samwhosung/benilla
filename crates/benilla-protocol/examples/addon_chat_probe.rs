//! Live probe: what an addon broadcast looks like on the wire. 1.12.1 has no addon opcode;
//! `SendAddonMessage` rides the ordinary chat lanes on their ordinary type byte, and only
//! `language == LANG_ADDON` (`0xFFFFFFFF`) marks it. vmangos relays the language verbatim
//! (`ChatHandler.cpp:488`) and skips `SanitizeChatMessage` for addon chat
//! (`ChatHandler.cpp:49`), so the prefix TAB survives.
//!
//! Run: `cargo run -p benilla-protocol --example addon_chat_probe -- probeN <password> [probeM
//! <password>] [host]`. One account runs the CHANNEL case; a second adds the PARTY case, and must
//! be an account nobody else is logged in on, since a login kicks the current holder.
//!
//! Needs `AddonChannel = 1` (off drops every addon send) and
//! `AllowTwoSide.Interaction.Channel = 0` (on, `Channel::Say` rewrites the language to
//! `LANG_UNIVERSAL`).

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use benilla_protocol::messages::{
    channel_notice, CHAT_MSG_CHANNEL, CHAT_MSG_PARTY, CHAT_TYPE_CHANNEL, CHAT_TYPE_PARTY,
    LANGUAGE_ADDON,
};
use benilla_protocol::{ServerPacket, WorldSession, WORLD_PORT};

/// A private channel, so the probe's own membership is the only one.
const PROBE_CHANNEL: &str = "benillaaddonprobe";

/// What a real Quiver 3.1.4 broadcasts: this prefix, a TAB, then the body.
const ADDON_PREFIX: &str = "Quiver";
const ADDON_BODY: &str = "VERSION:3.1.4";

/// The probe account's character: `probe4` plays `Probefour`.
fn probe_char_name(user: &str) -> Option<String> {
    let n: usize = user.strip_prefix("probe")?.parse().ok()?;
    let spelled = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ]
    .get(n)?;
    Some(format!("Probe{spelled}"))
}

fn login(host: &str, user: &str, pass: &str) -> Result<(WorldSession, String)> {
    let name = probe_char_name(user).context("account must be a probeN")?;
    let logon = benilla_protocol::logon(host, user, pass)?;
    let addr = logon
        .realms
        .first()
        .map(|r| r.address.clone())
        .unwrap_or_else(|| format!("{host}:{WORLD_PORT}"));
    let mut session = WorldSession::connect(&addr, user, logon.session_key)?;
    let character = session
        .char_enum()?
        .into_iter()
        .find(|c| c.name == name)
        .with_context(|| format!("{user} has no character {name}"))?;
    session.player_login(character.guid)?;
    session.set_active_mover(character.guid)?;
    session.set_read_timeout(Some(Duration::from_millis(200)))?;
    // Until the player is in world the server silently drops every `STATUS_LOGGEDIN` opcode
    // (`WorldSession.cpp:568-577`); a lost join only shows later, as `NOT_MEMBER` on the say.
    if !wait_for(&mut session, 30, |p| {
        matches!(p, ServerPacket::LoginVerifyWorld { .. })
    }) {
        bail!(
            "{name} never got SMSG_LOGIN_VERIFY_WORLD — not in world, every send would be dropped"
        );
    }
    // Start ungrouped: a group left by a crashed run turns every invite into
    // `SMSG_PARTY_COMMAND_RESULT`. Disbanding with no group is a no-op.
    session.group_disband()?;
    println!("probe: logged in {name} ({user}) — in world, ungrouped");
    Ok((session, name))
}

/// A read timeout on a quiet socket, the one expected recv error.
fn is_timeout(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            )
        })
    })
}

/// Drain until a packet satisfies `done`, or `secs` elapse.
fn wait_for(
    session: &mut WorldSession,
    secs: u64,
    mut done: impl FnMut(&ServerPacket) -> bool,
) -> bool {
    let trace = std::env::var("WOW_ADDON_PROBE_TRACE").is_ok();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        match session.recv() {
            Ok(packet) => {
                if trace {
                    println!("probe:   .. {}", packet.name());
                }
                if let ServerPacket::Notification { text } = &packet {
                    println!("probe:   <- NOTIFICATION {text:?}");
                }
                if done(&packet) {
                    return true;
                }
            }
            Err(e) if trace && !is_timeout(&e) => println!("probe:   !! recv: {e:#}"),
            Err(_) => {}
        }
    }
    false
}

/// One inbound line the probe cared about.
struct Seen {
    chat_type: u8,
    language: u32,
    text: String,
}

/// Drain for `secs`, keeping each `SMSG_MESSAGECHAT` whose text is in `wanted`.
/// `WOW_ADDON_PROBE_TRACE=1` prints every packet that goes past.
fn collect(session: &mut WorldSession, secs: u64, wanted: &[&str]) -> Vec<Seen> {
    let trace = std::env::var("WOW_ADDON_PROBE_TRACE").is_ok();
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let Ok(packet) = session.recv() else { continue };
        if trace {
            println!("probe:   .. {}", packet.name());
        }
        match packet {
            ServerPacket::MessageChat(m) => {
                if wanted.iter().any(|w| m.text == *w) {
                    println!(
                        "probe:   <- type {:#04x} language {:#010x} text {:?}",
                        m.chat_type, m.language, m.text
                    );
                    seen.push(Seen {
                        chat_type: m.chat_type,
                        language: m.language,
                        text: m.text,
                    });
                } else if trace {
                    println!(
                        "probe:      (other chat: type {:#04x} lang {:#010x} {:?})",
                        m.chat_type, m.language, m.text
                    );
                }
            }
            ServerPacket::Notification { text } => println!("probe:   <- NOTIFICATION {text:?}"),
            ServerPacket::ChannelNotify(n) => println!(
                "probe:   <- CHANNEL_NOTIFY notice {:#04x} on {:?}",
                n.notice, n.channel
            ),
            _ => {}
        }
    }
    seen
}

/// Judge one lane: the addon line carries `LANG_ADDON`, the lane's own type byte and its TAB;
/// the control line carries the same type byte and a real language.
fn judge(lane: &str, expect_type: u8, addon_text: &str, control_text: &str, seen: &[Seen]) -> bool {
    let mut ok = true;
    let mut check = |label: &str, cond: bool| {
        println!(
            "probe:   [{}] {lane} {label}",
            if cond { "PASS" } else { "FAIL" }
        );
        ok &= cond;
    };
    match seen.iter().find(|s| s.text == addon_text) {
        Some(a) => {
            check("addon line arrived", true);
            check(
                &format!("addon language is LANG_ADDON (got {:#010x})", a.language),
                a.language == LANGUAGE_ADDON,
            );
            check(
                &format!(
                    "addon rides the lane's own type byte {expect_type:#04x} (got {:#04x})",
                    a.chat_type
                ),
                a.chat_type == expect_type,
            );
            check(
                "addon payload keeps its TAB byte-for-byte",
                a.text == addon_text && a.text.contains('\t'),
            );
        }
        None => check("addon line arrived", false),
    }
    match seen.iter().find(|s| s.text == control_text) {
        Some(c) => {
            check("control line arrived", true);
            check(
                &format!(
                    "control speaks a real tongue, not LANG_ADDON (got {:#010x})",
                    c.language
                ),
                c.language != LANGUAGE_ADDON,
            );
            check(
                &format!(
                    "control rides the same type byte {expect_type:#04x} (got {:#04x})",
                    c.chat_type
                ),
                c.chat_type == expect_type,
            );
        }
        None => check("control line arrived", false),
    }
    ok
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: addon_chat_probe -- <probeN> <password> [<probeM> <password>] [host]";
    let (user_a, pass_a) = match args.as_slice() {
        [a, b, ..] => (a.clone(), b.clone()),
        _ => bail!("{usage}"),
    };
    // An optional second account, then an optional host, told apart by the `probe` prefix.
    let second = match args.get(2) {
        Some(u) if u.starts_with("probe") => Some((u.clone(), args.get(3).context(usage)?.clone())),
        _ => None,
    };
    let host = args
        .get(if second.is_some() { 4 } else { 2 })
        .cloned()
        .unwrap_or_else(|| "localhost".into());

    let addon_text = format!("{ADDON_PREFIX}\t{ADDON_BODY}");
    let mut all_ok = true;
    // Await-invite mode: wait to be invited, accept and broadcast, so a benilla started with
    // `WOW_PROBE_CHAT="/invite <this character>"` gets a party member whose addon talks.
    let await_invite = std::env::var("WOW_ADDON_PROBE_AWAIT_INVITE").is_ok();

    // ── 1 · the CHANNEL lane, single session ──────────────────────────────────────────────────
    // `Channel::SendToAll` includes the sender (`Channel.cpp:777-785`): one session round-trips.
    let (mut a, name_a) = login(&host, &user_a, &pass_a)?;
    if await_invite {
        println!("probe: — PARTY lane — {name_a} waits to be invited (60s)");
        if !wait_for(&mut a, 60, |p| {
            matches!(p, ServerPacket::GroupInvite { .. })
        }) {
            bail!("no SMSG_GROUP_INVITE in 60s — did the other client send `/invite {name_a}`?");
        }
        a.group_accept()?;
        if !wait_for(&mut a, 20, |p| matches!(p, ServerPacket::GroupList { .. })) {
            bail!("accepted but never got SMSG_GROUP_LIST — the group did not form");
        }
        println!("probe:   grouped — broadcasting");
        let party_control = "party control line";
        a.send_addon_message(CHAT_TYPE_PARTY, None, &addon_text)?;
        a.send_chat_kind(CHAT_TYPE_PARTY, None, party_control)?;
        // Party chat echoes to its sender (`Group::BroadcastPacket` takes no ignore guid).
        let seen = collect(&mut a, 8, &[&addon_text, party_control]);
        all_ok &= judge("PARTY", CHAT_MSG_PARTY, &addon_text, party_control, &seen);
        a.group_disband()?;
        println!("---");
        if all_ok {
            println!("VERDICT: PASS — broadcast sent and relayed; the other client's log is the other half");
            return Ok(());
        }
        bail!("VERDICT: FAIL — see the per-check lines above");
    }
    println!("probe: — CHANNEL lane — {name_a} joins {PROBE_CHANNEL:?}");
    a.join_channel(PROBE_CHANNEL, "")?;
    if !wait_for(&mut a, 20, |p| {
        matches!(p, ServerPacket::ChannelNotify(n)
            if n.channel.eq_ignore_ascii_case(PROBE_CHANNEL) && n.notice == channel_notice::YOU_JOINED)
    }) {
        bail!("CHANNEL lane: no YOU_JOINED for {PROBE_CHANNEL:?} — the join never took");
    }
    println!("probe:   joined {PROBE_CHANNEL:?}");
    let chan_control = "channel control line";
    a.send_addon_message(CHAT_TYPE_CHANNEL, Some(PROBE_CHANNEL), &addon_text)?;
    a.send_chat_kind(CHAT_TYPE_CHANNEL, Some(PROBE_CHANNEL), chan_control)?;
    let seen = collect(&mut a, 5, &[&addon_text, chan_control]);
    all_ok &= judge(
        "CHANNEL",
        CHAT_MSG_CHANNEL,
        &addon_text,
        chan_control,
        &seen,
    );
    a.leave_channel(PROBE_CHANNEL)?;

    // ── 2 · the PARTY lane, two sessions ──────────────────────────────────────────────────────
    match second {
        None => println!(
            "probe: — PARTY lane — SKIPPED (pass a second probe account to run the reported case)"
        ),
        Some((user_b, pass_b)) => {
            let (mut b, name_b) = login(&host, &user_b, &pass_b)?;
            println!("probe: — PARTY lane — {name_a} invites {name_b}");
            a.group_invite(&name_b)?;
            if !wait_for(&mut b, 20, |p| {
                matches!(p, ServerPacket::GroupInvite { .. })
            }) {
                bail!("PARTY lane: {name_b} never saw SMSG_GROUP_INVITE — is {name_a} already grouped?");
            }
            b.group_accept()?;
            // Wait for the roster on both sides: a `/p` sent before the sender's own
            // `SMSG_GROUP_LIST` is dropped by `if (!group) return`.
            if !wait_for(&mut b, 20, |p| matches!(p, ServerPacket::GroupList { .. })) {
                bail!("PARTY lane: {name_b} accepted but never got SMSG_GROUP_LIST");
            }
            if !wait_for(&mut a, 20, |p| matches!(p, ServerPacket::GroupList { .. })) {
                bail!("PARTY lane: {name_a} never got SMSG_GROUP_LIST — the group did not form");
            }
            println!("probe:   {name_a} + {name_b} are grouped");

            let party_control = "party control line";
            a.send_addon_message(CHAT_TYPE_PARTY, None, &addon_text)?;
            a.send_chat_kind(CHAT_TYPE_PARTY, None, party_control)?;
            let seen = collect(&mut b, 5, &[&addon_text, party_control]);
            all_ok &= judge("PARTY", CHAT_MSG_PARTY, &addon_text, party_control, &seen);

            a.group_disband()?;
            b.group_disband()?;
        }
    }

    println!("---");
    if all_ok {
        println!(
            "VERDICT: PASS — an addon broadcast reaches the client as an ORDINARY chat type with \
             language {LANGUAGE_ADDON:#010x} and its TAB intact; the language field is the whole \
             discriminator"
        );
        Ok(())
    } else {
        bail!(
            "VERDICT: FAIL — see the per-check lines above. If EVERY addon line is missing, check \
             `AddonChannel = 1` in mangosd.conf; if the CHANNEL addon line arrived with language \
             0x00000000, `AllowTwoSide.Interaction.Channel` is on and rewrote it."
        )
    }
}
