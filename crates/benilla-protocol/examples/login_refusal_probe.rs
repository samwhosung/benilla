//! Live probe: what a refused character login looks like on the wire, and what it leaves. It
//! checks that `SMSG_CHARACTER_LOGIN_FAILED` (0x41) arrives with result `1`
//! (`HandlePlayerLoginOpcode`), which the 1.12 client reads as `CHAR_LOGIN_NO_WORLD` ("World
//! server is down"); that the session stays `STATUS_AUTHED` and still serves `CMSG_CHAR_ENUM`;
//! and that a valid pick then works on the same socket.
//!
//! A creature guid trips the stateless `!packet.guid.IsPlayer()` guard. Needs the local vmangos
//! and an account in `WOW_USER`/`WOW_PASS`.

use std::time::Duration;

use benilla_protocol::{logon, messages::ServerPacket, WorldSession, WORLD_PORT};

/// A `HIGHGUID_UNIT` guid (`0xF130…`): `ObjectGuid::IsPlayer()` rejects it before any lookup.
const NOT_A_PLAYER: u64 = 0xF130_0000_0000_0001;

fn connect(user: &str, pass: &str) -> anyhow::Result<WorldSession> {
    let l = logon("localhost", user, pass)?;
    let addr = l
        .realms
        .first()
        .map(|r| r.address.clone())
        .unwrap_or_else(|| format!("localhost:{WORLD_PORT}"));
    WorldSession::connect(&addr, user, l.session_key)
}

fn main() -> anyhow::Result<()> {
    let user = std::env::var("WOW_USER").map_err(|_| anyhow::anyhow!("set WOW_USER"))?;
    let pass = std::env::var("WOW_PASS").map_err(|_| anyhow::anyhow!("set WOW_PASS"))?;
    let (user, pass) = (user.as_str(), pass.as_str());
    let mut s = connect(user, pass)?;
    let chars = s.char_enum()?;
    println!(
        "roster: {:?}",
        chars.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    let target = chars.first().expect("the probe account has a character");

    // ── 1 · the refusal, and its byte ──
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.player_login(NOT_A_PLAYER)?;
    let mut refusal = None;
    for _ in 0..64 {
        match s.recv()? {
            ServerPacket::CharacterLoginFailed { result } => {
                refusal = Some(result);
                break;
            }
            other => println!("  (skipping {} while waiting)", other.name()),
        }
    }
    match refusal {
        Some(result) => println!("PASS: SMSG_CHARACTER_LOGIN_FAILED result = {result:#04x}"),
        None => anyhow::bail!("FAIL: no SMSG_CHARACTER_LOGIN_FAILED for a non-player guid"),
    }

    // ── 2 · the session survives it ──
    let again = s.char_enum()?;
    println!(
        "PASS: post-refusal roster still serves {} char(s)",
        again.len()
    );

    // ── 3 · a valid pick still works on the same socket ──
    s.player_login(target.guid)?;
    s.set_active_mover(target.guid)?;
    let mut entered = false;
    for _ in 0..64 {
        if let ServerPacket::LoginVerifyWorld { map, .. } = s.recv()? {
            println!("PASS: retry entered the world (SMSG_LOGIN_VERIFY_WORLD, map {map})");
            entered = true;
            break;
        }
    }
    if !entered {
        anyhow::bail!("FAIL: the retry never reached SMSG_LOGIN_VERIFY_WORLD");
    }
    Ok(())
}
