//! Live probe: a Horde character's chat reaches vmangos in its own language. A say must echo
//! back, a dot-command must answer (it passes the pre-parse `KnowsLanguage` gate), and the split
//! writer must carry the language too. The server drops a say in a language the character does
//! not know, answering only `SMSG_NOTIFICATION`.
//!
//! Run: `cargo run -p benilla-protocol --example horde_chat_probe -- probeN <password> [host]`.
//! Creates the orc `Orc<N-spelled>` on the account on first run.

use anyhow::{bail, Context, Result};
use benilla_protocol::messages::{CharCreateReq, CHAR_CREATE_NAME_IN_USE, CHAR_CREATE_SUCCESS};
use benilla_protocol::{ServerPacket, WorldSession, WORLD_PORT};

/// The account's orc: `probe4` plays `Orcfour`. Names are letters only and realm-unique.
fn orc_name(user: &str) -> Option<String> {
    let n: usize = user.strip_prefix("probe")?.parse().ok()?;
    let spelled = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ]
    .get(n)?;
    Some(format!("Orc{spelled}"))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let user = args
        .next()
        .context("usage: horde_chat_probe -- <probeN> <password> [host] (a probe account)")?;
    let pass = args
        .next()
        .context("usage: horde_chat_probe -- <probeN> <password> [host] (a probe account)")?;
    let host = args.next().unwrap_or_else(|| "localhost".into());
    let orc = orc_name(&user)
        .context("account must be a probeN (docs/CONTRIBUTING.md, \"Running it unattended\")")?;

    let logon = benilla_protocol::logon(&host, &user, &pass)?;
    let addr = logon
        .realms
        .first()
        .map(|r| r.address.clone())
        .unwrap_or_else(|| format!("{host}:{WORLD_PORT}"));
    let mut session = WorldSession::connect(&addr, &user, logon.session_key)?;

    let mut characters = session.char_enum()?;
    if !characters.iter().any(|c| c.name == orc) {
        // Race 2 orc, class 1 warrior, gender 0 male.
        let req = CharCreateReq {
            name: orc.clone(),
            race: 2,
            class: 1,
            gender: 0,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
        };
        match session.create_character(&req)? {
            CHAR_CREATE_SUCCESS | CHAR_CREATE_NAME_IN_USE => {}
            other => bail!("char create failed: {other:#x}"),
        }
        characters = session.char_enum()?;
    }
    let orc = characters
        .iter()
        .find(|c| c.name == orc)
        .context("slot orc exists after create")?;
    println!("probe: logging in {} (race {})", orc.name, orc.race);
    session.player_login(orc.guid)?;
    session.set_active_mover(orc.guid)?;

    // 1. An accepted say comes back from the server; the 1.12 client never echoes it locally.
    session.send_chat("orcish probe line")?;
    // 2. A dot-command passes the language gate: `.gps` answers with system messages.
    session.send_chat(".gps")?;

    let mut say_echoed = false;
    let mut system_reply = false;
    let mut notified: Option<String> = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !(say_echoed && system_reply) {
        match session.recv() {
            Ok(ServerPacket::MessageChat(m)) => {
                println!(
                    "probe: chat type {} lang {} text {:?}",
                    m.chat_type, m.language, m.text
                );
                if m.text == "orcish probe line" {
                    say_echoed = true;
                    println!("probe: SAY ECHOED (language {})", m.language);
                }
                // `CHAT_MSG_SYSTEM` is 0x0a; the text check skips the login MOTD, also SYSTEM.
                if m.chat_type == 0x0a && m.text.contains("Map:") {
                    system_reply = true;
                    println!("probe: .gps REPLY: {:?}", m.text);
                }
            }
            Ok(ServerPacket::Notification { text }) => {
                println!("probe: NOTIFICATION: {text:?}");
                notified = Some(text);
            }
            Ok(_) => {}
            Err(e) => bail!("recv: {e:#}"),
        }
    }

    // 3. The same through the split writer the app sends on; `into_split` keeps the language.
    let (mut reader, mut writer) = session.into_split()?;
    writer.send_chat("orcish writer line")?;
    let mut writer_echoed = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !writer_echoed {
        match reader.recv() {
            Ok(ServerPacket::MessageChat(m)) if m.text == "orcish writer line" => {
                writer_echoed = true;
                println!("probe: WRITER SAY ECHOED (language {})", m.language);
            }
            Ok(ServerPacket::Notification { text }) => {
                println!("probe: NOTIFICATION: {text:?}");
                notified = Some(text);
            }
            Ok(_) => {}
            Err(e) => bail!("reader recv: {e:#}"),
        }
    }

    println!("---");
    println!("say_echoed    = {say_echoed}");
    println!("system_reply  = {system_reply}");
    println!("writer_echoed = {writer_echoed}");
    println!("notification  = {notified:?}");
    if say_echoed && system_reply && writer_echoed && notified.is_none() {
        println!("VERDICT: PASS — Horde chat + dot-commands accepted on both send paths");
        Ok(())
    } else {
        bail!("VERDICT: FAIL");
    }
}
